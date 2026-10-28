//! Hub screen bodies. Each screen describes an optional metric strip, heading lines, one table
//! and footer lines; `draw_list` lays them out inside the window.

use super::*;
use crate::ui::panels::section;
use crate::ui::spans::{bar, button, compact, duration, keycap, sparkline, tag};

#[derive(Default)]
pub(super) struct ListView {
    pub strip: Vec<Metric>,
    pub head: Vec<Line<'static>>,
    pub table: Option<TableView>,
    pub foot: Vec<Line<'static>>,
}

pub(super) struct TableView {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Line<'static>>>,
    pub groups: Vec<(usize, String)>,
    pub selected: Option<usize>,
}

/// What one legacy bonus gives a new run, from its config fields.
fn bonus_effect(spec: &hoshi_core::PrestigeBonusSpec) -> String {
    let mut parts = Vec::new();
    let start = [
        (spec.starting_metal, "Fe"),
        (spec.starting_crystal, "Si"),
        (spec.starting_deuterium, "D"),
    ]
    .into_iter()
    .filter(|(amount, _)| *amount > 0.0)
    .map(|(amount, sym)| format!("{sym} {amount:.0}"))
    .collect::<Vec<_>>();
    if !start.is_empty() {
        parts.push(format!("start with {}", start.join(" ")));
    }
    if spec.production > 0.0 {
        parts.push(format!("+{:.0}% production", spec.production * 100.0));
    }
    if spec.research > 0.0 {
        parts.push(format!("+{:.0}% research speed", spec.research * 100.0));
    }
    if spec.queue_slots > 0 {
        parts.push(format!("+{} build queue slot", spec.queue_slots));
    }
    if parts.is_empty() {
        "no effect".into()
    } else {
        parts.join(", ")
    }
}

/// A leader trait as its name and what it changes: `Steady +10% output`.
fn trait_effect(spec: &hoshi_core::TraitSpec) -> String {
    let effects = [
        (spec.production, "output"),
        (spec.attack, "attack"),
        (spec.shield, "shields"),
        (spec.policy, "policy"),
    ]
    .into_iter()
    .filter(|(factor, _)| (factor - 1.0).abs() > 1e-9)
    .map(|(factor, what)| format!("{:+.0}% {what}", (factor - 1.0) * 100.0))
    .collect::<Vec<_>>();
    if effects.is_empty() {
        format!("{} (no bonus)", spec.name)
    } else {
        format!("{} {}", spec.name, effects.join(" "))
    }
}

/// One decimal place without a negative zero: `-0.04` reads `0.0`.
fn one_decimal(v: f64) -> String {
    let text = format!("{v:.1}");
    if text == "-0.0" { "0.0".into() } else { text }
}

/// Turns a snake_case key into words with a capital first letter.
pub(super) fn humanize(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn metric(
    theme: &Theme,
    label: &str,
    value: impl Into<String>,
    intent: Intent,
    foot: impl Into<String>,
) -> Metric {
    let value_style = match intent {
        Intent::Neutral => theme.strong(),
        other => theme.intent(other).add_modifier(Modifier::BOLD),
    };
    Metric {
        label: label.to_string(),
        value: Line::styled(value.into(), value_style),
        foot: Line::styled(foot.into(), theme.muted()),
    }
}

fn callout_lines(theme: &Theme, intent: Intent, title: &str, detail: &str) -> Vec<Line<'static>> {
    let accent = theme.intent(intent);
    let mut lines = vec![Line::from(vec![
        Span::styled("▌ ", accent),
        Span::styled(title.to_string(), accent.add_modifier(Modifier::BOLD)),
    ])];
    if !detail.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▌ ", accent),
            Span::styled(detail.to_string(), theme.muted()),
        ]));
    }
    lines
}

/// Resolved incidents stay listed this long after they close.
const RECENT_INCIDENT_SECS: u64 = 24 * 60 * 60;

impl App {
    /// Dims the frame and centres a dialog sized to its wrapped content.
    pub(super) fn dialog(
        &self,
        frame: &mut Frame<'_>,
        title: &str,
        subtitle: String,
        lines: Vec<Line<'static>>,
        hints: &[(String, &str)],
    ) {
        let t = &self.theme;
        let area = frame.area();
        ui::chrome::dim(frame.buffer_mut(), area, t);
        let width = area.width.saturating_sub(4).min(84).max(area.width.min(40));
        let text_w = width.saturating_sub(4).max(1) as usize;
        let rows: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(text_w))
            .sum();
        let height = (rows as u16 + 4).min(area.height.saturating_sub(2)).max(3);
        let rect = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, rect);
        let hints: Vec<(String, String)> = hints
            .iter()
            .map(|(key, label)| (key.clone(), label.to_string()))
            .collect();
        let dialog = Dialog {
            theme: t,
            title: title.to_string(),
            subtitle,
            close_hint: self.keymap.primary(Binding::Cancel),
            footer_note: Some(Line::from(ui::spans::key_hints(t, &hints))),
            buttons: Vec::new(),
        };
        let inner = dialog.inner(rect);
        frame.render_widget(dialog, rect);
        let body = Rect {
            x: inner.x + 2,
            y: inner.y + 1,
            width: inner.width.saturating_sub(4),
            height: inner.height.saturating_sub(1),
        };
        frame.render_widget(
            Paragraph::new(lines)
                .style(t.text())
                .wrap(Wrap { trim: false }),
            body,
        );
    }

    pub(super) fn draw_list(&self, frame: &mut Frame<'_>, area: Rect) {
        let t = &self.theme;
        let view = self.list_view();
        let strip_h = if view.strip.is_empty() {
            0
        } else {
            MetricStrip::HEIGHT + 1
        };
        let has_table = view.table.is_some();
        let gap = u16::from(has_table);
        let head_h = if view.head.is_empty() {
            0
        } else {
            view.head.len() as u16 + gap
        };
        let foot_h = if view.foot.is_empty() {
            0
        } else {
            view.foot.len() as u16 + gap
        };
        let rest = area.height.saturating_sub(strip_h);
        let (head_h, foot_h) = if has_table {
            let budget = rest.saturating_sub(4);
            let head_h = head_h.min(budget);
            (head_h, foot_h.min(budget.saturating_sub(head_h)))
        } else {
            (
                rest.saturating_sub(foot_h.min(rest / 2)),
                foot_h.min(rest / 2),
            )
        };
        let [strip, head, body, foot] = Layout::vertical([
            Constraint::Length(strip_h),
            Constraint::Length(head_h),
            Constraint::Min(0),
            Constraint::Length(foot_h),
        ])
        .areas(area);
        if strip_h > 0 {
            frame.render_widget(
                MetricStrip {
                    theme: t,
                    metrics: view.strip,
                },
                Rect {
                    height: MetricStrip::HEIGHT,
                    ..strip
                },
            );
        }
        frame.render_widget(Paragraph::new(view.head).style(t.text()), head);
        if let Some(table) = view.table {
            frame.render_widget(
                DataTable {
                    theme: t,
                    columns: table.columns,
                    rows: table.rows,
                    selected: table.selected,
                    striped: true,
                    surface: Surface::Raised,
                    groups: table.groups,
                },
                body,
            );
        }
        let foot_area = Rect {
            y: foot.y + gap.min(foot.height),
            height: foot.height.saturating_sub(gap),
            ..foot
        };
        frame.render_widget(Paragraph::new(view.foot).style(t.text()), foot_area);
    }

    pub(super) fn list_view(&self) -> ListView {
        match self.screen {
            Screen::Planet => self.planet_view(),
            Screen::Shipyard => self.shipyard_view(),
            Screen::Overview => self.research_view(),
            Screen::Wonder => self.wonder_view(),
            Screen::Prestige => self.prestige_view(),
            Screen::Achievements => self.achievement_view(),
            Screen::Daily => self.daily_view(),
            Screen::Goals => self.goals_view(),
            Screen::Production => self.production_view(),
            Screen::Market => self.market_view(),
            Screen::Diplomacy => self.diplomacy_view(),
            Screen::Laws => self.law_view(),
            Screen::Reports => self.report_view(),
            Screen::History => self.history_view(),
            Screen::Staff => self.staff_view(),
            Screen::Ops => self.ops_view(),
            Screen::Automation => self.automation_view(),
            Screen::Finance => self.finance_view(),
            Screen::Kpis => self.kpi_view(),
            Screen::Incidents => self.incident_view(),
            Screen::Maintenance => self.maintenance_view(),
            Screen::Map => self.map_view(),
            Screen::Cockpit => ListView::default(),
            Screen::Scenarios => self.scenario_view(),
        }
    }

    fn chip(&self, text: &str, intent: Intent, minimal: bool) -> Line<'static> {
        Line::from(tag(&self.theme, &text.to_uppercase(), intent, minimal))
    }

    fn status_chip(&self, reason: Option<&str>) -> Line<'static> {
        match reason {
            None => self.chip("ready", Intent::Success, false),
            Some(reason) => self.chip(reason, Intent::Warning, true),
        }
    }

    fn row_style(&self, blocked: bool) -> Style {
        if blocked {
            self.theme.label().add_modifier(Modifier::DIM)
        } else {
            self.theme.text()
        }
    }

    fn cost_cells(&self, cost: Resources, stock: Resources, base: Style) -> Vec<Line<'static>> {
        let t = &self.theme;
        ResourceKind::QUOTED
            .iter()
            .map(|&res| {
                let amount = cost.get(res);
                if amount < 0.5 {
                    Line::styled("—", t.faint())
                } else if amount > stock.get(res) {
                    Line::styled(compact(amount), t.intent(Intent::Danger))
                } else {
                    Line::styled(compact(amount), base)
                }
            })
            .collect()
    }

    fn cost_columns(columns: &mut Vec<Column>) {
        for res in ResourceKind::QUOTED {
            columns.push(Column::right(res.abbr(), 4));
        }
    }

    fn progress_cell(&self, frac: f64, text: String, intent: Intent) -> Line<'static> {
        let mut spans = bar(&self.theme, frac, 12, intent);
        spans.push(Span::styled(format!(" {text}"), self.theme.muted()));
        Line::from(spans)
    }

    fn props(&self, pairs: &[(&str, String)]) -> Line<'static> {
        let t = &self.theme;
        let mut spans = Vec::new();
        for (i, (label, value)) in pairs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("   ", t.faint()));
            }
            spans.push(Span::styled(
                format!("{} ", label.to_uppercase()),
                t.label(),
            ));
            spans.push(Span::styled(value.clone(), t.text()));
        }
        Line::from(spans)
    }

    fn planet_view(&self) -> ListView {
        let coord = self.selected_coord();
        let levels = self
            .state
            .buildings_at(coord)
            .unwrap_or(&self.state.buildings);
        let stock = self
            .state
            .resources_at(coord)
            .copied()
            .unwrap_or(self.state.resources);
        let rows = BuildingKind::ALL
            .iter()
            .map(|&kind| {
                let reason = self.building_block_reason(kind);
                let base = self.row_style(reason.is_some());
                let cost = self.state.build_cost_on(coord, kind);
                let mut row = vec![
                    Line::styled(humanize(&kind.to_string()), base),
                    Line::styled(levels.get(kind).to_string(), base),
                ];
                row.extend(self.cost_cells(cost, stock, base));
                row.push(Line::styled(
                    duration(self.state.build_time_secs_on(coord, kind)),
                    base,
                ));
                row.push(self.status_chip(reason.as_deref()));
                row
            })
            .collect();
        let mut columns = vec![Column::left("building", 18), Column::right("lv", 2)];
        Self::cost_columns(&mut columns);
        columns.push(Column::right("time", 7));
        columns.push(Column::fill("status"));
        ListView {
            head: self.planet_info_lines(),
            table: Some(TableView {
                columns,
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn shipyard_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let stock = self.state.resources;
        let mut head = vec![self.props(&[(
            "queue",
            format!(
                "{}/{}",
                self.state.ship_queue.len(),
                self.state.config.max_ship_queue
            ),
        )])];
        if let Some((cost, secs)) = self.shipyard_batch(self.ship_batch) {
            head.push(Line::from(vec![
                Span::styled("batch ", t.label()),
                keycap(t, "-"),
                Span::styled(format!(" ×{} ", self.ship_batch), t.strong()),
                keycap(t, "+"),
                Span::styled(
                    format!("  {}  {}  ", cost.brief(), duration(secs)),
                    t.text(),
                ),
                keycap(t, "space"),
                Span::styled(" max", t.muted()),
            ]));
        }
        for (i, job) in self.state.ship_queue.iter().enumerate() {
            let timer = job
                .completes_at
                .map(|at| duration(at.saturating_sub(now)))
                .unwrap_or_else(|| "waiting".into());
            head.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), t.label()),
                Span::styled(format!("{} ×{}", job.label(), job.remaining), t.text()),
                Span::styled(format!("  {timer}"), t.muted()),
            ]));
        }
        let mut rows = Vec::new();
        for &kind in ShipKind::ALL.iter() {
            let spec = self.state.config.ships.get(kind);
            let cost = spec.unit_cost();
            let reason = self.ship_block_reason(kind, cost);
            let base = self.row_style(reason.is_some());
            let mut row = vec![
                Line::styled(humanize(&kind.to_string()), base),
                Line::styled(self.state.fleet.get(kind).to_string(), base),
            ];
            row.extend(self.cost_cells(cost, stock, base));
            row.push(Line::styled(duration(spec.duration_secs()), base));
            row.push(Line::styled(
                format!("{:.0}", self.state.ship_stats(kind).attack),
                base,
            ));
            row.push(self.status_chip(reason.as_deref()));
            rows.push(row);
        }
        for &kind in DefenceKind::ALL.iter() {
            let spec = self.state.config.defences.get(kind);
            let cost = spec.unit_cost();
            let reason = self.defence_block_reason(kind, cost);
            let base = self.row_style(reason.is_some());
            let mut row = vec![
                Line::styled(humanize(&kind.to_string()), base),
                Line::styled(self.state.defences.get(kind).to_string(), base),
            ];
            row.extend(self.cost_cells(cost, stock, base));
            row.push(Line::styled(duration(spec.duration_secs()), base));
            row.push(Line::styled(format!("{:.0}", spec.attack), base));
            row.push(self.status_chip(reason.as_deref()));
            rows.push(row);
        }
        let mut columns = vec![Column::left("unit", 16), Column::right("#", 4)];
        Self::cost_columns(&mut columns);
        columns.push(Column::right("time", 7));
        columns.push(Column::right("atk", 5));
        columns.push(Column::fill("status"));
        ListView {
            head,
            table: Some(TableView {
                columns,
                rows,
                groups: vec![
                    (0, "ships".into()),
                    (ShipKind::ALL.len(), "defences".into()),
                ],
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn research_view(&self) -> ListView {
        let t = &self.theme;
        let mut tabs = Vec::new();
        for branch in self.visible_tree_branches() {
            tabs.push(button(t, branch.as_key(), branch == self.tree_branch));
            tabs.push(Span::raw(" "));
        }
        tabs.push(Span::styled("  h/l branch", t.muted()));
        let specs = self.overview_techs();
        let mut groups = Vec::new();
        let mut last_group = None;
        let mut rows = Vec::new();
        for (i, spec) in specs.iter().enumerate() {
            // Keystones close a branch and open its victory path.
            let group = if spec.keystone {
                "keystone ★ victory path".to_string()
            } else {
                format!("tier {}", spec.tier)
            };
            if last_group.as_ref() != Some(&group) {
                groups.push((i, group.clone()));
                last_group = Some(group);
            }
            let level = self.state.tech_level(&spec.id);
            let next = self.state.next_research_level(&spec.id);
            let cost = spec.cost(next);
            let reason = self.tech_block_reason(&spec.id, cost);
            let base = self.row_style(reason.is_some());
            let mut name = vec![
                if spec.keystone {
                    Span::styled("★ ", t.intent(Intent::Warning))
                } else {
                    Span::raw("  ")
                },
                Span::styled(spec.name.clone(), base),
            ];
            if level > 0 {
                name.push(Span::styled(format!(" {level}"), t.muted()));
            }
            let mut row = vec![Line::from(name)];
            row.extend(self.cost_cells(cost, self.state.resources, base));
            row.push(Line::styled(duration(spec.duration_secs(next)), base));
            let node = self.state.tech_node_state(&spec.id);
            row.push(match node {
                TechNodeState::InProgress => self.chip("in progress", Intent::Warning, false),
                TechNodeState::Excluded => self.chip("excluded", Intent::Neutral, true),
                _ => self.status_chip(reason.as_deref()),
            });
            rows.push(row);
        }
        let mut columns = vec![Column::fill("tech")];
        Self::cost_columns(&mut columns);
        columns.push(Column::right("time", 7));
        // Wide enough for "need research lab 1", which used to read NEED RESEARC.
        columns.push(Column::left("status", 20));
        ListView {
            head: vec![Line::from(tabs)],
            table: Some(TableView {
                columns,
                rows,
                groups,
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn wonder_view(&self) -> ListView {
        let t = &self.theme;
        let head = match &self.state.wonder {
            Some(job) => {
                let spec = self.state.config.wonders.iter().find(|s| s.id == job.id);
                let name = spec.map(|s| s.name.as_str()).unwrap_or(job.id.as_str());
                let total = spec.map(|s| s.stages.len()).unwrap_or(0);
                let remaining = self.state.wonder_remaining().unwrap_or(Resources::ZERO);
                let eta = self
                    .state
                    .wonder_eta_secs()
                    .map(duration)
                    .unwrap_or_else(|| "unknown".into());
                callout_lines(
                    t,
                    Intent::Primary,
                    &format!("Building {name} · stage {}/{total}", job.stage + 1),
                    &format!("{} · eta {eta}", missing_label(remaining)),
                )
            }
            None => vec![self.props(&[("active", "none".into())])],
        };
        let rows = self
            .state
            .config
            .wonders
            .iter()
            .map(|spec| {
                let reason = self.wonder_block_reason(spec);
                let base = self.row_style(reason.is_some());
                let active = self.state.wonder.as_ref().is_some_and(|j| j.id == spec.id);
                let status = match reason.as_deref() {
                    Some("done") => self.chip("done", Intent::Success, true),
                    _ if active => self.chip("building", Intent::Primary, false),
                    other => self.status_chip(other),
                };
                vec![
                    Line::styled(spec.name.clone(), base),
                    Line::styled(spec.stages.len().to_string(), base),
                    status,
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("wonder", 28),
                    Column::right("stages", 6),
                    Column::fill("status"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn prestige_view(&self) -> ListView {
        let t = &self.theme;
        let preview = self.state.prestige_preview();
        let cfg = &self.state.config.prestige;
        let points = self.state.prestige.currency;
        let strip = vec![
            metric(t, "RUN", preview.run.index.to_string(), Intent::Neutral, ""),
            metric(
                t,
                "SCORE",
                preview.score.to_string(),
                Intent::Neutral,
                "levels + colonies + wonders",
            ),
            metric(
                t,
                "EARNED",
                preview.earned.to_string(),
                Intent::Neutral,
                format!("points on reset, 1 per {}", cfg.points_per_currency.max(1)),
            ),
            metric(
                t,
                "POINTS",
                points.to_string(),
                Intent::Neutral,
                "legacy points to spend",
            ),
        ];
        let mut head = Vec::new();
        if self.confirming_prestige {
            let accent = t.intent(Intent::Danger);
            for text in [
                format!("Reset keeps: {}", preview.kept.join(", ")),
                format!("Reset loses: {}", preview.lost.join(", ")),
            ] {
                head.push(Line::from(vec![
                    Span::styled("▌ ", accent),
                    Span::styled(text, t.text()),
                ]));
            }
            head.push(Line::from(vec![
                Span::styled("▌ ", accent),
                Span::styled(
                    "Enter again to confirm. x cancels.",
                    accent.add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        let mut rows = vec![vec![
            Line::styled("Reset galaxy", t.strong()),
            Line::default(),
            Line::default(),
            if preview.eligible {
                self.chip("ready", Intent::Success, false)
            } else {
                self.chip("locked", Intent::Warning, true)
            },
            Line::styled(
                if preview.eligible {
                    format!("start over with {} more points", preview.earned)
                } else {
                    format!(
                        "needs score {} (now {}), a wonder or a victory",
                        cfg.min_score, preview.score
                    )
                },
                t.muted(),
            ),
        ]];
        for spec in &cfg.bonuses {
            let owned = self.state.prestige.owned_count(&spec.id);
            rows.push(vec![
                Line::styled(spec.name.clone(), t.text()),
                Line::styled(spec.cost.to_string(), t.text()),
                Line::styled(owned.to_string(), t.text()),
                if points >= spec.cost {
                    self.chip("ready", Intent::Success, false)
                } else {
                    self.chip(
                        &format!("need {}", spec.cost - points),
                        Intent::Warning,
                        true,
                    )
                },
                Line::styled(bonus_effect(spec), t.muted()),
            ]);
        }
        ListView {
            strip,
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("item", 24),
                    Column::right("cost", 5),
                    Column::right("owned", 5),
                    Column::left("status", 10),
                    Column::fill("effect"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn achievement_view(&self) -> ListView {
        let t = &self.theme;
        let earned = self.state.achievements.len();
        let total = self.state.config.achievements.len();
        let frac = if total == 0 {
            0.0
        } else {
            earned as f64 / total as f64
        };
        let mut spans = vec![
            Span::styled("EARNED ", t.label()),
            Span::styled(format!("{earned}/{total} "), t.strong()),
        ];
        spans.extend(bar(t, frac, 20, Intent::Success));
        let mut head = vec![Line::from(spans)];
        head.push(Line::styled(
            "Progress counts what this save already has, including starting buildings and faction standing.",
            t.muted(),
        ));
        // Achievements stop on a modified save; say so instead of showing
        // targets that were passed but never earned.
        let blocked = self.state.is_modified();
        if blocked {
            head.push(Line::styled(
                "Feats are off: cheats modified this save.",
                t.intent(Intent::Warning),
            ));
        }
        let rows = self
            .state
            .config
            .achievements
            .iter()
            .map(|spec| {
                let unlocked = self.state.achievements.contains_key(&spec.id);
                if unlocked {
                    vec![
                        Line::styled(spec.listed_title(true).to_string(), t.text()),
                        self.progress_cell(1.0, "complete".into(), Intent::Success),
                        self.chip("earned", Intent::Success, false),
                        Line::styled(spec.description.clone(), t.muted()),
                    ]
                } else if spec.hidden {
                    vec![
                        Line::styled("Hidden feat", t.faint()),
                        Line::styled("—", t.faint()),
                        self.chip("hidden", Intent::Neutral, true),
                        Line::styled("revealed when earned", t.faint()),
                    ]
                } else {
                    let current = spec.condition.current(&self.state);
                    let target = spec.condition.target();
                    let frac = if target > 0.0 { current / target } else { 0.0 };
                    vec![
                        Line::styled(spec.listed_title(false).to_string(), t.text()),
                        self.progress_cell(
                            frac,
                            format!("{current:.0}/{target:.0}"),
                            Intent::Primary,
                        ),
                        if blocked {
                            self.chip("blocked", Intent::Warning, true)
                        } else {
                            self.chip("open", Intent::Neutral, true)
                        },
                        Line::styled(spec.description.clone(), t.muted()),
                    ]
                }
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("feat", 22),
                    Column::left("progress", 20),
                    Column::left("state", 9),
                    Column::fill("needs"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn daily_view(&self) -> ListView {
        let t = &self.theme;
        let streak = self.state.daily.streak;
        let cap = self.state.config.daily.streak_cap;
        let head = vec![self.props(&[
            ("streak", format!("{streak}/{cap}")),
            ("resets in", duration(self.state.daily_resets_in())),
        ])];
        let rows = self
            .state
            .daily
            .slots
            .iter()
            .map(|slot| {
                let name = self
                    .state
                    .config
                    .daily
                    .get(&slot.spec_id)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| humanize(&slot.spec_id));
                let frac = if slot.target == 0 {
                    0.0
                } else {
                    slot.progress as f64 / slot.target as f64
                };
                let done = slot.done();
                vec![
                    Line::styled(name, t.text()),
                    self.progress_cell(
                        frac,
                        format!("{}/{}", slot.progress, slot.target),
                        if done {
                            Intent::Success
                        } else {
                            Intent::Primary
                        },
                    ),
                    if done {
                        self.chip("done", Intent::Success, false)
                    } else {
                        self.chip("open", Intent::Neutral, true)
                    },
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("task", 26),
                    Column::left("progress", 24),
                    Column::fill("state"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn goals_view(&self) -> ListView {
        let t = &self.theme;
        let ladder = self.state.ladder_progress();
        let progress = self.state.victory_progress();
        let paths = [
            ("Conquest", &progress.conquest),
            ("Wonder", &progress.wonder),
            ("Diplomacy", &progress.diplomacy),
        ];
        let strip = paths
            .iter()
            .map(|(name, path)| {
                let value = format!("{:.0}%", path.ratio * 100.0);
                let label = name.to_uppercase();
                if path.ready {
                    metric(t, &label, value, Intent::Success, "ready")
                } else {
                    metric(t, &label, value, Intent::Neutral, path.next_step.clone())
                }
            })
            .collect();
        let mut head = match &ladder.next {
            Some(goal) => callout_lines(
                t,
                Intent::Primary,
                &format!("Next  {}  {:.0}%", goal.name, goal.ratio * 100.0),
                &goal.next_step,
            ),
            None => callout_lines(t, Intent::Success, "Next  ladder complete", ""),
        };
        let current = ladder
            .current
            .as_deref()
            .map_or_else(|| "none".into(), |id| self.state.ladder_tier_name(id));
        let mut facts = vec![("ladder", current)];
        match &self.state.victory {
            Some(record) => facts.push((
                "won via",
                format!("{}  +{} prestige", record.path.label(), record.prestige),
            )),
            None => facts.push(("galaxy victory", "first path wins".into())),
        }
        if let Some(coord) = self.state.pirate_homeworld() {
            facts.push(("pirate homeworld", coord.to_string()));
        }
        head.push(self.props(&facts));
        let rows = paths
            .iter()
            .map(|(name, path)| {
                vec![
                    Line::styled(name.to_string(), t.text()),
                    self.progress_cell(
                        path.ratio,
                        format!("{:.0}%", path.ratio * 100.0),
                        if path.ready {
                            Intent::Success
                        } else {
                            Intent::Primary
                        },
                    ),
                    if path.ready {
                        self.chip("ready", Intent::Success, false)
                    } else {
                        Line::default()
                    },
                    Line::styled(path.next_step.clone(), t.muted()),
                ]
            })
            .collect();
        ListView {
            strip,
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("path", 10),
                    Column::left("progress", 18),
                    Column::left("state", 7),
                    Column::fill("next step"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn production_view(&self) -> ListView {
        let t = &self.theme;
        let report = self.state.production_report();
        let coord = self.selected_coord();
        let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { 0.0 };
        let energy_short = report.energy_demand > report.energy_supply;
        let heat_short = report.heat_load > report.heat_sink;
        let limited = report.limited_by.as_key();
        let intent_if = |bad: bool| if bad { Intent::Danger } else { Intent::Success };
        let mut rate = metric(
            t,
            "Fe /h",
            format!("{:.0}", report.metal_per_hour),
            Intent::Neutral,
            "",
        );
        rate.foot = Line::from(sparkline(t, &self.prod_history, 16, Intent::Primary));
        let strip = vec![
            metric(
                t,
                "ENERGY",
                format!("{:.0}/{:.0}", report.energy_supply, report.energy_demand),
                intent_if(energy_short),
                format!(
                    "{:.0}% drawn",
                    ratio(report.energy_demand, report.energy_supply) * 100.0
                ),
            ),
            metric(
                t,
                "HEAT",
                format!("{:.0}/{:.0}", report.heat_load, report.heat_sink),
                intent_if(heat_short),
                format!(
                    "{:.0}% of sink",
                    ratio(report.heat_load, report.heat_sink) * 100.0
                ),
            ),
            metric(
                t,
                "LIMIT",
                // How many chains are held back; the line below names the worst.
                match report
                    .steps
                    .iter()
                    .filter(|step| step.limited_by != hoshi_core::LimitingFactor::None)
                    .count()
                {
                    0 if limited == "none" => "clear".into(),
                    0 => humanize(limited),
                    n => format!("{n} stalled"),
                },
                if limited == "none" {
                    Intent::Success
                } else {
                    Intent::Warning
                },
                report.limited_by.suggestion().to_string(),
            ),
            rate,
        ];
        let head = vec![Line::styled(
            format!(
                "Fe {:.0}  Si {:.0}  REE {:.0}  H2O {:.0}  He3 {:.0}  D {:.0}  per hour",
                report.metal_per_hour,
                report.crystal_per_hour,
                report.rare_earths_per_hour,
                report.water_ice_per_hour,
                report.helium_3_per_hour,
                report.deuterium_per_hour
            ),
            t.muted(),
        )];
        let goods = |items: &std::collections::BTreeMap<String, f64>, digits: usize| {
            items
                .iter()
                .map(|(id, n)| format!("{} {n:.digits$}", id.replace('_', " ")))
                .collect::<Vec<_>>()
                .join("  ")
        };
        let rows = report
            .steps
            .iter()
            .filter(|step| step.planet == coord)
            .map(|step| {
                let limit = step.limited_by.as_key();
                vec![
                    Line::styled(humanize(&step.id), t.text()),
                    Line::styled(goods(&step.outputs_per_hour, 1), t.text()),
                    if limit == "none" {
                        self.chip("ok", Intent::Success, true)
                    } else {
                        self.chip(&limit.replace('_', " "), Intent::Warning, true)
                    },
                    Line::styled(goods(&step.buffers, 0), t.muted()),
                ]
            })
            .collect();
        ListView {
            strip,
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("recipe", 18),
                    Column::left("out /h", 18),
                    Column::left("limit", 10),
                    Column::fill("buffer"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn market_view(&self) -> ListView {
        let t = &self.theme;
        let market = &self.state.market;
        let fee_pct = self.state.effective_market_fee() * 100.0;
        // A trade carries at most the free allowance or docked cargo, so
        // `max` stops there even with more in stock.
        let cargo = self
            .state
            .config
            .market
            .cargo_free
            .max(self.state.cargo_capacity(self.state.fleet));
        let head = vec![Line::from(vec![
            Span::styled("AMOUNT ", t.label()),
            keycap(t, "-"),
            Span::styled(format!(" {:.0} ", self.trade_amount), t.strong()),
            keycap(t, "+"),
            Span::raw(" "),
            keycap(t, &self.keymap.primary(Binding::Toggle)),
            Span::styled(format!(" max · cargo {cargo:.0}"), t.muted()),
            Span::styled("   FEE ", t.label()),
            Span::styled(format!("{fee_pct:.0}%"), t.text()),
            Span::styled("   HUB ", t.label()),
            Span::styled(self.state.config.market.hub().to_string(), t.text()),
        ])];
        let rows = TRADE_PAIRS
            .iter()
            .map(|&(pay, recv)| {
                let pair = Line::styled(format!("{} → {}", pay.abbr(), recv.abbr()), t.text());
                match self.state.quote_market(pay, recv, self.trade_amount) {
                    Ok(quote) => {
                        let ratios: Vec<f64> = market
                            .history(pay)
                            .iter()
                            .zip(market.history(recv))
                            .filter(|(_, r)| **r > 0.0)
                            .map(|(p, r)| p / r)
                            .collect();
                        let delta = match (ratios.first(), ratios.last()) {
                            (Some(first), Some(last)) if *first > 0.0 => {
                                let pct = (last / first - 1.0) * 100.0;
                                let intent = if pct >= 0.0 {
                                    Intent::Success
                                } else {
                                    Intent::Danger
                                };
                                Line::styled(format!("{pct:+.1}%"), t.intent(intent))
                            }
                            _ => Line::styled("—", t.faint()),
                        };
                        let rate = if quote.pay_amount > 0.0 {
                            quote.recv_amount / quote.pay_amount
                        } else {
                            0.0
                        };
                        vec![
                            pair,
                            Line::styled(format!("{rate:.2}"), t.text()),
                            Line::from(sparkline(t, &ratios, 12, Intent::Primary)),
                            delta,
                            match self.state.trade_blocker(&quote) {
                                // Same check as the trade itself, so Enter cannot surprise.
                                Some(blocker) => Line::styled(
                                    format!(
                                        "{:.1} {} · {}",
                                        quote.recv_amount,
                                        recv.abbr(),
                                        blocker.to_string().trim_start_matches("market: ")
                                    ),
                                    t.intent(Intent::Warning),
                                ),
                                None => Line::styled(
                                    format!("{:.1} {}", quote.recv_amount, recv.abbr()),
                                    t.strong(),
                                ),
                            },
                        ]
                    }
                    Err(e) => vec![
                        pair,
                        Line::styled("—", t.faint()),
                        Line::default(),
                        Line::default(),
                        Line::styled(e.to_string(), t.intent(Intent::Warning)),
                    ],
                }
            })
            .collect();
        let mut foot = vec![
            self.props(&[
                ("orders", self.state.board.orders.len().to_string()),
                ("contracts", self.state.board.contracts.len().to_string()),
                (
                    "shipments",
                    self.state
                        .board
                        .shipments
                        .values()
                        .filter(|s| !s.delivered)
                        .count()
                        .to_string(),
                ),
            ]),
        ];
        if let Some(region) = self.state.board.regions.get(&self.state.home) {
            foot.push(Line::from(vec![
                Span::styled("HOME BOOK  ", t.label()),
                Span::styled("Si ", t.muted()),
                sparkline(
                    t,
                    region.history(ResourceKind::Silicon),
                    16,
                    Intent::Primary,
                ),
                Span::styled("  D ", t.muted()),
                sparkline(
                    t,
                    region.history(ResourceKind::Deuterium),
                    16,
                    Intent::Primary,
                ),
            ]));
        }
        let now = self.state.last_tick_unix;
        let mut contracts: Vec<_> = self.state.board.contracts.values().collect();
        // Contracts the player can still act on come before settled ones.
        contracts.sort_by_key(|c| (c.status.is_terminal(), std::cmp::Reverse(c.id)));
        for contract in contracts.into_iter().take(3) {
            let status = contract.status.as_key();
            let due = if contract.deadline > now {
                format!("due in {}", ui::spans::duration(contract.deadline - now))
            } else {
                format!("due {} ago", ui::spans::duration(now - contract.deadline))
            };
            foot.push(Line::styled(
                format!(
                    "contract #{} {} {:.0} {} {status} {due}",
                    contract.id,
                    contract.kind.as_key(),
                    contract.remaining,
                    contract.good.label(),
                ),
                t.muted(),
            ));
        }
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("pair", 8),
                    Column::right("rate", 6),
                    Column::left("trend", 12),
                    Column::right("Δ", 7),
                    Column::fill("you get"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            foot,
            ..ListView::default()
        }
    }

    /// Dispatch form: target, ship steppers, flight time and fuel.
    pub(super) fn dispatch_lines(&self, form: &DispatchForm) -> Vec<Line<'static>> {
        let t = &self.theme;
        let dest = form.dest;
        let place = if self.state.owned_coords().contains(&dest) {
            "colony"
        } else if let Some(out) = self.state.outpost(dest) {
            out.kind.as_key()
        } else if let Some(site) = self.state.site_at(dest) {
            site.kind.as_key()
        } else if self
            .state
            .frontier
            .visible_worlds()
            .any(|w| w.coord == dest)
        {
            "frontier"
        } else {
            "empty slot"
        };
        let mut lines = vec![
            section(t, "dispatch"),
            Line::from(vec![
                Span::styled(format!("to {dest}"), t.strong()),
                Span::raw("  "),
                tag(t, &place.to_uppercase(), Intent::Neutral, true),
            ]),
            Line::from({
                let mut spans = vec![Span::styled("mission ", t.muted())];
                for mission in self.dispatch_missions(dest) {
                    if *mission == form.mission {
                        spans.push(tag(t, mission.label(), Intent::Primary, true));
                    } else {
                        spans.push(Span::styled(format!(" {} ", mission.label()), t.faint()));
                    }
                }
                spans
            }),
            Line::default(),
            section(t, "ships"),
        ];
        let w = ShipKind::ALL
            .iter()
            .map(|k| humanize(&k.to_string()).len())
            .max()
            .unwrap_or(0);
        for (i, &kind) in ShipKind::ALL.iter().enumerate() {
            let have = self.state.fleet.get(kind);
            let picked = form.ships.get(kind);
            let on = i == form.selected;
            let name_style = if on {
                t.strong()
            } else if have == 0 {
                t.faint()
            } else {
                t.text()
            };
            let mut spans = vec![
                Span::styled(if on { "▌" } else { " " }, t.intent(Intent::Primary)),
                Span::styled(format!("{:<w$} ", humanize(&kind.to_string())), name_style),
            ];
            if on {
                spans.push(keycap(t, "-"));
                spans.push(Span::styled(format!(" {picked:>3} "), t.strong()));
                spans.push(keycap(t, "+"));
            } else {
                spans.push(Span::styled(
                    format!("   {picked:>3}    "),
                    if picked > 0 { t.text() } else { t.faint() },
                ));
            }
            spans.push(Span::styled(format!("  of {have}"), t.muted()));
            lines.push(Line::from(spans));
        }
        lines.push(Line::default());
        if form.ships.is_empty() {
            lines.extend(callout_lines(
                t,
                Intent::Primary,
                "pick ships with +/-",
                "j/k moves between ship types.",
            ));
        } else {
            match self.state.flight_time_and_fuel(form.ships, dest, 100) {
                Ok((eta, fuel)) => {
                    lines.push(
                        self.props(&[("ETA", duration(eta)), ("fuel", format!("{fuel:.1} D"))]),
                    );
                    if form.mission == Mission::Expedition {
                        let hold = self.state.config.expedition.min_hold_secs;
                        let weight = self.state.expedition_pirate_weight(&form.ships, hold);
                        lines.push(self.props(&[("pirate weight", weight.to_string())]));
                    }
                    if fuel > self.state.resources.deuterium {
                        lines.extend(callout_lines(
                            t,
                            Intent::Danger,
                            "not enough deuterium",
                            &format!("have {:.0} D", self.state.resources.deuterium),
                        ));
                    }
                }
                Err(e) => lines.extend(callout_lines(t, Intent::Danger, &e.to_string(), "")),
            }
        }
        lines.push(Line::default());
        let mut keys = vec![keycap(t, "⏎"), Span::styled(" launch  ", t.muted())];
        if self.dispatch_missions(dest).len() > 1 {
            keys.extend([keycap(t, "space"), Span::styled(" mission  ", t.muted())]);
        }
        keys.extend([keycap(t, "x"), Span::styled(" close", t.muted())]);
        lines.push(Line::from(keys));
        lines
    }

    fn standing_gauge(&self, standing: f64, min: f64, max: f64) -> Line<'static> {
        let t = &self.theme;
        const W: usize = 15;
        let span = (max - min).max(1.0);
        let at = |v: f64| (((v - min) / span).clamp(0.0, 1.0) * (W - 1) as f64).round() as usize;
        let (zero, pos) = (at(0.0), at(standing));
        let intent = if standing >= 0.0 {
            Intent::Success
        } else {
            Intent::Danger
        };
        Line::from(
            (0..W)
                .map(|i| {
                    if i == pos {
                        Span::styled("◆", t.intent(intent))
                    } else if i == zero {
                        Span::styled("┼", t.line())
                    } else {
                        Span::styled("─", t.faint())
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

    fn diplomacy_view(&self) -> ListView {
        let t = &self.theme;
        let cfg = &self.state.config.diplomacy;
        let head = if cfg.enabled {
            vec![self.props(&[
                (
                    "decay",
                    format!("{:.0}% / {}", cfg.decay * 100.0, duration(cfg.tick_secs)),
                ),
                (
                    "standing",
                    format!("{}..{}", cfg.min_standing, cfg.max_standing),
                ),
            ])]
        } else {
            callout_lines(t, Intent::Neutral, "Diplomacy off", "")
        };
        let rows = cfg
            .iter()
            .map(|spec| {
                let standing = self.state.diplomacy.standing(&spec.id);
                let tier = spec.tier(standing).map(|t| t.name.as_str()).unwrap_or("-");
                let next = match spec.next_tier(standing) {
                    Some(t) => format!("{} {:.0}", t.name, t.min),
                    None => "max".into(),
                };
                let tribute = if spec.tribute_metal > 0.0 {
                    format!("{:.0} Fe → {:+.0}", spec.tribute_metal, spec.tribute_delta)
                } else {
                    "—".into()
                };
                vec![
                    Line::styled(spec.name.clone(), t.text()),
                    self.standing_gauge(standing, cfg.min_standing, cfg.max_standing),
                    Line::styled(format!("{standing:.0}"), t.strong()),
                    self.chip(tier, Intent::Neutral, true),
                    Line::styled(next, t.muted()),
                    Line::styled(tribute, t.muted()),
                ]
            })
            .collect();
        let mut foot = vec![section(t, "recent")];
        if self.state.diplomacy.log.is_empty() {
            foot.push(Line::styled("none", t.faint()));
        }
        for change in self.state.diplomacy.log.iter().rev().take(4) {
            let intent = if change.delta >= 0.0 {
                Intent::Success
            } else {
                Intent::Danger
            };
            foot.push(Line::from(vec![
                Span::styled(format!("{} ", change.faction), t.text()),
                Span::styled(format!("{:+.0} ", change.delta), t.intent(intent)),
                Span::styled(change.reason.to_string(), t.muted()),
            ]));
        }
        if self.state.rivals.enabled {
            foot.push(section(t, "rivals"));
            for row in self.state.rival_intel() {
                let stock = match row.resources {
                    Some(r) => format!(
                        "  Fe {}  Si {}  D {}",
                        compact(r.iron),
                        compact(r.silicon),
                        compact(r.deuterium)
                    ),
                    None => String::new(),
                };
                let fleet = match row.fleet {
                    Some(f) => format!("  fleet {}", f.get(ShipKind::LightFighter)),
                    None => String::new(),
                };
                foot.push(Line::from(vec![
                    Span::styled(format!("{} ", row.name), t.text()),
                    Span::styled(
                        format!(
                            "{} {} last {}  {}  conf {:.2}  age {}{stock}{fleet}",
                            row.home,
                            row.personality,
                            row.last_action,
                            row.source,
                            row.confidence,
                            row.age_secs
                        ),
                        t.muted(),
                    ),
                ]));
            }
        }
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("faction", 14),
                    Column::left("standing", 15),
                    Column::right("now", 4),
                    Column::left("tier", 10),
                    Column::left("next", 14),
                    Column::fill("tribute"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            foot,
            ..ListView::default()
        }
    }

    fn law_view(&self) -> ListView {
        let t = &self.theme;
        let head = vec![Line::styled(
            "Enter proposes, approves or repeals. Samples compare expected with measured.",
            t.muted(),
        )];
        let mut rows = Vec::new();
        for spec in self.state.config.laws.iter() {
            rows.push(vec![
                Line::styled(spec.title.clone(), t.text()),
                Line::styled(
                    format!("{} {:.2}", spec.param_name.replace('_', " "), spec.default),
                    t.muted(),
                ),
                self.chip(
                    match spec.jurisdiction {
                        JurisdictionKind::Empire => "empire",
                        JurisdictionKind::Planet => "planet",
                    },
                    Intent::Neutral,
                    true,
                ),
                self.chip("propose", Intent::Primary, true),
            ]);
        }
        let templates = rows.len();
        for law in &self.state.laws.laws {
            let status = law.status.as_key();
            let intent = match status {
                "active" => Intent::Success,
                "proposed" => Intent::Warning,
                _ => Intent::Neutral,
            };
            let mut state = vec![tag(t, &status.to_uppercase(), intent, false)];
            for sample in self.state.laws.latest_samples(law.id) {
                state.push(Span::styled(
                    format!(
                        "  {} exp {:.1} act {:.1}",
                        sample.metric.as_key().replace('_', " "),
                        sample.expected,
                        sample.actual
                    ),
                    t.muted(),
                ));
            }
            rows.push(vec![
                Line::styled(
                    format!("#{} {}", law.id, humanize(&law.template_id)),
                    t.text(),
                ),
                Line::styled(format!("param {:.2}", law.param), t.muted()),
                self.chip(&law.jurisdiction.as_key(), Intent::Neutral, true),
                Line::from(state),
            ]);
        }
        let mut groups = vec![(0, "templates".to_string())];
        if rows.len() > templates {
            groups.push((templates, "enacted".into()));
        }
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("law", 22),
                    Column::left("parameter", 20),
                    Column::left("scope", 12),
                    Column::fill("state"),
                ],
                rows,
                groups,
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn map_view(&self) -> ListView {
        let t = &self.theme;
        let cursor = self.map_cursor();
        let legend = [
            ("H", "home", Intent::Primary),
            ("C", "colony", Intent::Success),
            ("P", "pirate", Intent::Danger),
            ("F", "frontier", Intent::Warning),
            ("f", "fleet", Intent::Neutral),
        ];
        let mut head = vec![self.props(&[
            ("galaxy", cursor.galaxy.to_string()),
            ("cursor", cursor.to_string()),
        ])];
        let mut legend_spans = Vec::new();
        for (glyph, name, intent) in legend {
            legend_spans.push(Span::styled(
                glyph,
                t.intent(intent).add_modifier(Modifier::BOLD),
            ));
            legend_spans.push(Span::styled(format!(" {name}   "), t.muted()));
        }
        head.push(Line::from(legend_spans));
        let owned = self.state.owned_coords();
        let now = self.state.last_tick_unix;
        let mut rows = Vec::new();
        for system in 1..=MAP_SYSTEM_MAX {
            let mut slots = Vec::new();
            let mut labels = Vec::new();
            for pos in 1..=MAP_SLOT_COUNT {
                let coord = Coord {
                    galaxy: cursor.galaxy,
                    system,
                    position: pos,
                };
                let (ch, intent) = if coord == self.state.home {
                    ('H', Some(Intent::Primary))
                } else if owned.contains(&coord) {
                    ('C', Some(Intent::Success))
                } else if self.state.outposts.iter().any(|o| o.coord == coord) {
                    ('P', Some(Intent::Danger))
                } else if self
                    .state
                    .frontier
                    .visible_worlds()
                    .any(|w| w.coord == coord)
                {
                    ('F', Some(Intent::Warning))
                } else if self.state.missions.iter().any(|m| m.dest == coord) {
                    ('f', Some(Intent::Neutral))
                } else {
                    ('·', None)
                };
                let style = intent.map_or(t.faint(), |i| t.intent(i).add_modifier(Modifier::BOLD));
                if pos == self.map_position && system == self.map_system {
                    slots.push(Span::styled(format!("[{ch}]"), t.selected()));
                } else {
                    slots.push(Span::styled(format!(" {ch} "), style));
                }
                if coord == self.state.home {
                    labels.push("home".to_string());
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
                    labels.push(format!(
                        "fleet #{} {} {}",
                        mission.id,
                        mission.phase.as_key(),
                        duration(mission.phase_ends_at.saturating_sub(now))
                    ));
                }
            }
            rows.push(vec![
                Line::styled(system.to_string(), t.label()),
                Line::from(slots),
                Line::styled(labels.join("  "), t.muted()),
            ]);
        }
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::right("sys", 3),
                    Column::left("slots", MAP_SLOT_COUNT * 3),
                    Column::fill("notes"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.map_system.saturating_sub(1) as usize),
            }),
            ..ListView::default()
        }
    }

    fn report_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        if self.state.outposts.is_empty() {
            return ListView {
                head: callout_lines(
                    t,
                    Intent::Primary,
                    "No pirate outposts",
                    "Outposts appear as the galaxy wakes up; probe them from here.",
                ),
                ..ListView::default()
            };
        }
        let rows = self
            .state
            .outposts
            .iter()
            .map(|outpost| {
                let target = Line::styled(outpost.coord.to_string(), t.text());
                let kind = self.chip(outpost.kind.as_key(), Intent::Danger, true);
                match self.state.latest_report(outpost.coord) {
                    Some(report) => {
                        let n = self.state.reports_for(outpost.coord).len();
                        let extra = if n > 1 {
                            format!(" +{}", n - 1)
                        } else {
                            String::new()
                        };
                        vec![
                            target,
                            kind,
                            Line::styled(format!("{}{extra}", report.tier), t.text()),
                            Line::styled(duration(report.age_secs(now)), t.muted()),
                            Line::styled(
                                format!("{:.2}", self.state.report_confidence(report)),
                                t.muted(),
                            ),
                            if report.detected {
                                self.chip("detected", Intent::Warning, false)
                            } else {
                                Line::default()
                            },
                        ]
                    }
                    None => vec![
                        target,
                        kind,
                        Line::styled("no report", t.faint()),
                        Line::styled("—", t.faint()),
                        Line::styled("—", t.faint()),
                        Line::default(),
                    ],
                }
            })
            .collect();
        ListView {
            table: Some(TableView {
                columns: vec![
                    Column::left("target", 8),
                    Column::left("kind", 10),
                    Column::left("report", 10),
                    Column::right("age", 7),
                    Column::right("conf", 5),
                    Column::fill("flags"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn history_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let kind = self
            .history_kind
            .map(|k| k.as_key().to_string())
            .unwrap_or_else(|| "all".into());
        let actor = self.history_actor.clone().unwrap_or_else(|| "all".into());
        let mut head = vec![self.props(&[
            ("type", kind),
            ("actor", actor),
            ("retained", self.state.history.events.len().to_string()),
        ])];
        let events = self.history_rows();
        if events.is_empty() {
            head.push(Line::default());
            head.extend(callout_lines(
                t,
                Intent::Primary,
                "No history yet",
                "Builds, trades, raids and decisions are logged here as they happen.",
            ));
            return ListView {
                head,
                ..ListView::default()
            };
        }
        let rows = events
            .iter()
            .map(|event| {
                vec![
                    Line::styled(format!("#{}", event.id), t.label()),
                    Line::styled(
                        format!("{} ago", duration(now.saturating_sub(event.at))),
                        t.muted(),
                    ),
                    self.chip(event.kind.as_key(), Intent::Neutral, true),
                    Line::styled(event.actor.clone(), t.text()),
                    Line::styled(
                        format!(
                            "{}  {}",
                            event.action.replace('_', " "),
                            event
                                .objects
                                .iter()
                                .map(|id| self.state.object_label(id))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        t.text(),
                    ),
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::right("id", 5),
                    Column::right("age", 11),
                    Column::left("type", 12),
                    Column::left("actor", 10),
                    Column::fill("event"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn ops_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let schedule = self.state.mission_schedule(now);
        let head = vec![self.props(&[
            ("vehicles", schedule.vehicles.len().to_string()),
            ("missions", schedule.missions.len().to_string()),
            ("alerts", schedule.alerts.len().to_string()),
        ])];
        if schedule.vehicles.is_empty() && schedule.missions.is_empty() {
            let mut head = head;
            head.push(Line::default());
            head.extend(callout_lines(
                t,
                Intent::Primary,
                "No vehicles registered",
                &format!(
                    "Press {} to register the ships parked at {} as a vehicle, then {} on it to plan a mission.",
                    self.keymap.primary(Binding::Increase),
                    self.selected_coord(),
                    self.keymap.primary(Binding::Confirm),
                ),
            ));
            return ListView {
                head,
                ..ListView::default()
            };
        }
        let mut rows = Vec::new();
        let mut groups = vec![(0, "vehicles".to_string())];
        for vehicle in &schedule.vehicles {
            let state = if vehicle.free_at > now {
                self.chip(
                    &format!("booked {}", duration(vehicle.free_at - now)),
                    Intent::Warning,
                    true,
                )
            } else {
                self.chip("idle", Intent::Success, true)
            };
            rows.push(vec![
                Line::styled(format!("#{} {}", vehicle.id, vehicle.name), t.text()),
                Line::styled(format!("at {}", vehicle.coord), t.muted()),
                state,
            ]);
        }
        groups.push((rows.len(), "missions".into()));
        for mission in &schedule.missions {
            rows.push(vec![
                Line::styled(format!("mission {}", mission.id), t.text()),
                Line::styled(
                    format!(
                        "{} → {}  {}",
                        mission.route.origin,
                        mission.route.dest,
                        mission.transfer.as_key()
                    ),
                    t.muted(),
                ),
                Line::styled(
                    format!("eta {}", duration(mission.arrives_at.saturating_sub(now))),
                    t.text(),
                ),
            ]);
        }
        groups.push((rows.len(), "alerts".into()));
        for alert in &schedule.alerts {
            let kind = match alert.kind {
                hoshi_core::ScheduleAlertKind::DoubleBook => "double-book",
                hoshi_core::ScheduleAlertKind::MissedWindow => "missed window",
            };
            rows.push(vec![
                Line::styled(format!("vehicle #{}", alert.vehicle_id), t.text()),
                Line::styled(alert.detail.clone(), t.muted()),
                self.chip(kind, Intent::Danger, false),
            ]);
        }
        groups.dedup_by_key(|(at, _)| *at);
        groups.retain(|(at, _)| *at < rows.len());
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("item", 22),
                    Column::left("detail", 28),
                    Column::fill("state"),
                ],
                rows,
                groups,
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn automation_view(&self) -> ListView {
        let t = &self.theme;
        let catalog = self.state.automation_catalog();
        let head = vec![self.props(&[
            (
                "control energy",
                format!("{:.0}", self.state.automation_control_energy()),
            ),
            ("interval", duration(catalog.interval_secs)),
        ])];
        let mut rows = Vec::new();
        for tier in &catalog.tier {
            // Name the unlocking tech as the Research screen does.
            let tech = self.state.display_name(&tier.research);
            let unlocked = self.state.automation_tier_unlocked(&tier.id);
            rows.push(vec![
                Line::styled(tier.name.clone(), t.text()),
                Line::styled(
                    if unlocked {
                        format!(
                            "{tech} {} researched · {:.0} energy",
                            tier.research_level, tier.energy
                        )
                    } else {
                        format!(
                            "research {tech} {} (have {})",
                            tier.research_level,
                            self.state.tech_level(&tier.research)
                        )
                    },
                    t.muted(),
                ),
                if unlocked {
                    self.chip("unlocked", Intent::Success, true)
                } else {
                    self.chip("locked", Intent::Neutral, true)
                },
            ]);
        }
        let tiers = rows.len();
        let preview = self.state.preview_automation();
        for rule in &self.state.automation_rules {
            let pre = preview
                .iter()
                .find(|p| p.rule_id == rule.id)
                .map(|p| p.detail.clone())
                .unwrap_or_default();
            rows.push(vec![
                Line::styled(
                    format!(
                        "#{} {} → {}",
                        rule.id,
                        rule.condition.describe(),
                        rule.action.describe()
                    ),
                    t.text(),
                ),
                Line::styled(pre, t.muted()),
                if rule.enabled {
                    self.chip("on", Intent::Success, false)
                } else {
                    self.chip("off", Intent::Neutral, true)
                },
            ]);
        }
        let mut groups = vec![(0, "tiers".to_string())];
        let mut foot = Vec::new();
        if rows.len() > tiers {
            groups.push((tiers, "rules".into()));
        } else {
            let tech = self.state.display_name("logistics_rules");
            foot = callout_lines(
                t,
                Intent::Primary,
                "No rules yet",
                &format!(
                    "Research {tech}, then hoshi automation add from the CLI. Enter dry-runs."
                ),
            );
        }
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("name", 30),
                    // Fits `research Logistics rules 1 (have 0)` and rule previews.
                    Column::left("detail", 40),
                    Column::fill("state"),
                ],
                rows,
                groups,
                selected: Some(self.selected),
            }),
            foot,
            ..ListView::default()
        }
    }

    fn kpi_view(&self) -> ListView {
        let t = &self.theme;
        let snap = self.state.kpi_snapshot();
        let due = snap
            .next_report_at
            .saturating_sub(self.state.last_tick_unix);
        let mut head = vec![self.props(&[
            ("next report", duration(due)),
            ("targets", snap.targets.len().to_string()),
            ("archived", snap.reports.len().to_string()),
        ])];
        if let Some(report) = snap.reports.last() {
            head.push(Line::styled(
                format!(
                    "period #{}  pending {}  {}",
                    report.id,
                    report.pending_decisions,
                    report.highlights.first().cloned().unwrap_or_default()
                ),
                t.muted(),
            ));
        }
        head.push(Line::styled(
            "var: value minus target · events: history rows behind the value",
            t.muted(),
        ));
        let no_missions = self
            .state
            .mission_schedule(self.state.last_tick_unix)
            .missions
            .is_empty();
        let rows = snap
            .measurements
            .iter()
            .map(|row| {
                let dash = || Line::styled("—", t.faint());
                // A ratio over zero missions is not a grade; show no value.
                let unsampled = row.kpi == hoshi_core::KpiId::MissionOnTime && no_missions;
                let var = match row.variance().filter(|_| !unsampled) {
                    // A variance that rounds to zero is on target, whatever its sign.
                    Some(v) if one_decimal(v) == "0.0" => {
                        Line::styled("0.0", t.intent(Intent::Success))
                    }
                    Some(v) => Line::styled(
                        format!("{v:+.1}"),
                        t.intent(if v > 0.0 {
                            Intent::Success
                        } else {
                            Intent::Danger
                        }),
                    ),
                    None => dash(),
                };
                let progress = match row.progress().filter(|_| !unsampled) {
                    Some(p) => self.progress_cell(
                        p,
                        format!("{:.0}%", p * 100.0),
                        if p >= 1.0 {
                            Intent::Success
                        } else {
                            Intent::Primary
                        },
                    ),
                    None => dash(),
                };
                vec![
                    Line::styled(humanize(row.kpi.as_key()), t.text()),
                    self.chip(&row.owner.as_key(), Intent::Neutral, true),
                    if unsampled {
                        dash()
                    } else {
                        Line::styled(one_decimal(row.value), t.strong())
                    },
                    row.target
                        .map(|v| Line::styled(one_decimal(v), t.text()))
                        .unwrap_or_else(dash),
                    var,
                    progress,
                    Line::styled(row.event_ids.len().to_string(), t.muted()),
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("kpi", 16),
                    Column::left("scope", 12),
                    Column::right("value", 7),
                    Column::right("target", 7),
                    Column::right("var", 6),
                    Column::left("progress", 18),
                    Column::right("events", 6),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn scenario_view(&self) -> ListView {
        let t = &self.theme;
        let Some(scenario) = self.scenario.as_ref() else {
            return ListView {
                head: callout_lines(
                    t,
                    Intent::Primary,
                    "No scenario",
                    "+ forks live into a sandbox. Queue a mine, space advances 1h, enter promotes.",
                ),
                ..ListView::default()
            };
        };
        let live = Scenario::create(&self.state).outcome();
        let plan = scenario.outcome();
        let head = vec![self.props(&[
            ("live", live.at.to_string()),
            ("sandbox", plan.at.to_string()),
            ("queued", scenario.planned().len().to_string()),
        ])];
        let rows: Vec<Vec<Line<'static>>> = self
            .scenario_compare_rows()
            .into_iter()
            .map(|(metric, left, right, conf)| {
                vec![
                    Line::styled(humanize(&metric), t.text()),
                    Line::styled(left, t.text()),
                    Line::styled(right, t.strong()),
                    if conf.is_empty() {
                        Line::styled("-", t.faint())
                    } else {
                        Line::styled(conf, t.muted())
                    },
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("metric", 22),
                    Column::right("live", 10),
                    Column::right("sandbox", 10),
                    Column::right("conf", 11),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn incident_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let snap = self.state.incident_snapshot();
        let mut head = vec![self.props(&[
            ("open", snap.open.len().to_string()),
            ("risks", snap.risks.len().to_string()),
            ("reviews", snap.reviews.len().to_string()),
        ])];
        let mut foot = Vec::new();
        if let Some(row) = snap.open.get(self.selected) {
            let objects = if row.objects.is_empty() {
                "-".into()
            } else {
                row.objects.join(",")
            };
            foot.push(Line::styled(
                format!("objects {objects}  events {}", row.event_ids.len()),
                t.muted(),
            ));
            if let Some(action) = row.actions.last() {
                foot.push(Line::styled(format!("last {action}"), t.muted()));
            }
            if let Some(review) = snap.reviews.iter().rev().find(|r| r.incident_id == row.id) {
                foot.push(Line::styled(
                    format!("review: {}", review.root_cause),
                    t.muted(),
                ));
            }
        }
        // A raid opens and closes its incident in one tick; the last day's
        // resolved incidents keep a visible trace.
        let mut recent: Vec<&hoshi_core::Incident> = self
            .state
            .incidents
            .incidents
            .iter()
            .filter(|row| {
                row.status == hoshi_core::IncidentStatus::Resolved
                    && row
                        .closed_at
                        .is_some_and(|at| now.saturating_sub(at) <= RECENT_INCIDENT_SECS)
            })
            .collect();
        recent.sort_by_key(|row| std::cmp::Reverse(row.closed_at));
        if !recent.is_empty() {
            foot.push(section(t, "recently resolved"));
            for row in recent.iter().take(4) {
                let ago = now.saturating_sub(row.closed_at.unwrap_or(now));
                foot.push(Line::from(vec![
                    Span::styled(
                        format!("#{} {}  ", row.id, humanize(row.kind.as_key())),
                        t.text(),
                    ),
                    tag(
                        t,
                        &row.severity.as_key().to_uppercase(),
                        Intent::Neutral,
                        true,
                    ),
                    Span::styled(
                        format!(
                            "  {}  {} ago",
                            row.resolution.as_deref().unwrap_or("resolved"),
                            duration(ago)
                        ),
                        t.muted(),
                    ),
                ]));
            }
        }
        foot.push(section(t, "reviews"));
        if snap.reviews.is_empty() {
            // One short line, readable at 80 columns.
            foot.push(Line::styled(
                "hoshi incidents review ID, after a high or critical one",
                t.muted(),
            ));
        } else {
            for review in snap.reviews.iter().rev().take(3) {
                foot.push(Line::styled(
                    format!("#{}  {}", review.incident_id, review.root_cause),
                    t.text(),
                ));
            }
        }
        if !snap.risks.is_empty() {
            foot.push(section(t, "risks"));
            for risk in snap.risks.iter().take(4) {
                foot.push(Line::from(vec![
                    Span::styled(format!("{}  ", risk.name), t.text()),
                    Span::styled(format!("L{:.2}  ", risk.likelihood), t.muted()),
                    tag(
                        t,
                        &risk.impact.as_key().to_uppercase(),
                        Intent::Neutral,
                        true,
                    ),
                    Span::styled(
                        format!(
                            "  review in {}",
                            duration(risk.review_at.saturating_sub(now))
                        ),
                        t.muted(),
                    ),
                ]));
            }
        }
        if snap.open.is_empty() {
            head.push(Line::default());
            head.extend(callout_lines(
                t,
                Intent::Success,
                "No open incidents",
                "Raids, heat overflow, missed contracts and leaks open one here.",
            ));
            // No table to show, so the sections take the whole screen instead
            // of the footer's half, and stay readable at 80x24.
            head.push(Line::default());
            head.append(&mut foot);
            return ListView {
                head,
                ..ListView::default()
            };
        }
        let rows = snap
            .open
            .iter()
            .map(|row| {
                let severity = row.severity.as_key();
                let intent = match severity {
                    "critical" | "high" => Intent::Danger,
                    "medium" => Intent::Warning,
                    _ => Intent::Neutral,
                };
                vec![
                    Line::styled(format!("#{}", row.id), t.label()),
                    Line::styled(humanize(row.kind.as_key()), t.text()),
                    self.chip(severity, intent, false),
                    self.chip(row.status.as_key(), Intent::Neutral, true),
                    Line::styled(
                        row.owner
                            .map(|id| format!("leader #{id}"))
                            .unwrap_or_else(|| "unassigned".into()),
                        t.muted(),
                    ),
                    Line::styled(row.event_ids.len().to_string(), t.muted()),
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::right("id", 4),
                    Column::left("kind", 18),
                    Column::left("severity", 10),
                    Column::left("status", 12),
                    Column::left("owner", 12),
                    Column::right("ev", 3),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            foot,
            ..ListView::default()
        }
    }

    fn maintenance_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let snap = self.state.maintenance_snapshot();
        let cfg = self.state.maintenance_config();
        let mut head = vec![self.props(&[
            ("assets", snap.assets.len().to_string()),
            ("orders", snap.orders.len().to_string()),
            ("readings", snap.readings.len().to_string()),
            (
                "next reading in",
                duration(snap.next_at.saturating_sub(now)),
            ),
        ])];
        head.push(Line::styled(
            "condition falls with run time; a service order takes the asset offline and restores it",
            t.muted(),
        ));
        let mut foot = Vec::new();
        if snap.assets.is_empty() {
            head.push(Line::default());
            head.extend(callout_lines(
                t,
                Intent::Neutral,
                "No assets yet",
                "Mines, defences and vehicles appear here as they come online.",
            ));
            return ListView {
                head,
                foot,
                ..ListView::default()
            };
        }
        if let Some(asset) = snap.assets.get(self.selected) {
            foot.push(section(t, &self.state.asset_label(asset)));
            if let Some(pred) = snap
                .predictions
                .iter()
                .rev()
                .find(|p| p.asset_id == asset.id)
            {
                foot.push(Line::styled(
                    format!(
                        "failure chance {:.1}% within {}",
                        pred.probability * 100.0,
                        duration(pred.horizon_secs)
                    ),
                    t.muted(),
                ));
            } else {
                let p = hoshi_core::predict_failure(
                    &cfg,
                    hoshi_core::fail_p_per_hour(&cfg, asset.hours_use, asset.condition, false),
                );
                foot.push(Line::styled(
                    format!(
                        "failure chance {:.1}% within {}",
                        p * 100.0,
                        duration(cfg.horizon_secs)
                    ),
                    t.muted(),
                ));
            }
            if let Some(reading) = snap.readings.iter().rev().find(|r| r.asset_id == asset.id) {
                foot.push(Line::styled(
                    format!(
                        "last reading: heat {:.0}% of sink  wear {:.0}%  power {:.0}% of rated",
                        reading.temperature * 100.0,
                        reading.vibration * 100.0,
                        reading.power * 100.0
                    ),
                    t.muted(),
                ));
            }
            for order in snap
                .orders
                .iter()
                .filter(|o| o.asset_id == asset.id)
                .take(2)
            {
                foot.push(Line::styled(
                    format!(
                        "order #{} {} ends {}",
                        order.id,
                        humanize(order.kind.as_key()),
                        duration(order.ends_at.saturating_sub(now))
                    ),
                    t.muted(),
                ));
            }
        }
        let rows = snap
            .assets
            .iter()
            .map(|asset| {
                let cond_intent = if asset.condition < 0.35 {
                    Intent::Danger
                } else if asset.condition < 0.7 {
                    Intent::Warning
                } else {
                    Intent::Success
                };
                let order = snap.orders.iter().find(|o| o.asset_id == asset.id);
                let status = match (order, asset.offline_until) {
                    (Some(order), _) => format!(
                        "offline {}",
                        duration(order.ends_at.saturating_sub(now).max(1))
                    ),
                    (_, Some(until)) if until > now => {
                        format!("offline {}", duration(until.saturating_sub(now)))
                    }
                    _ => "online".into(),
                };
                let status_intent = if status == "online" {
                    Intent::Success
                } else {
                    Intent::Warning
                };
                vec![
                    Line::styled(format!("#{}", asset.id), t.label()),
                    Line::styled(self.state.asset_label(asset), t.text()),
                    Line::styled(
                        format!("{:.0}%", asset.condition * 100.0),
                        t.intent(cond_intent),
                    ),
                    Line::styled(format!("{:.1}h", asset.hours_use), t.muted()),
                    self.chip(&status, status_intent, status == "online"),
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::right("id", 4),
                    Column::left("asset", 24),
                    Column::right("condition", 9),
                    Column::right("run time", 8),
                    Column::fill("status"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            foot,
            ..ListView::default()
        }
    }

    fn finance_view(&self) -> ListView {
        let t = &self.theme;
        let report = self.state.finance_report();
        let cfg = self.state.finance_config();
        let strip = vec![
            metric(
                t,
                "CASH",
                compact(report.cash),
                Intent::Neutral,
                format!("credits · debt {}", compact(report.debt)),
            ),
            metric(
                t,
                "INCOME",
                compact(report.income.total()),
                Intent::Success,
                format!(
                    "production {} · tax {} · trade {}",
                    compact(report.income.production),
                    compact(report.income.tax),
                    compact(report.income.trade)
                ),
            ),
            metric(
                t,
                "EXPENSE",
                compact(report.expense.total()),
                Intent::Danger,
                format!(
                    "industry {} · research {} · military {}",
                    compact(report.expense.industry),
                    compact(report.expense.research),
                    compact(report.expense.military)
                ),
            ),
            metric(
                t,
                "RESERVE",
                compact(report.reserve_target),
                if report.reserve_gap > 0.0 {
                    Intent::Warning
                } else {
                    Intent::Success
                },
                if report.reserve_gap > 0.0 {
                    format!("cash short of target by {}", compact(report.reserve_gap))
                } else {
                    "target met".into()
                },
            ),
        ];
        let head = vec![
            self.props(&[("period", duration(cfg.period_secs))]),
            Line::styled(
                "Credits come in from production, tax and trade and go out on spending. \
                 A spend over its budget limit waits here for approval.",
                t.muted(),
            ),
        ];
        let mut rows = Vec::new();
        for pending in &report.pending {
            rows.push(vec![
                Line::styled(
                    format!("#{} {}", pending.id, pending.center.as_key()),
                    t.text(),
                ),
                Line::default(),
                Line::styled(compact(pending.amount), t.strong()),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::from(vec![
                    tag(t, "HELD", Intent::Warning, false),
                    Span::styled(format!(" {}", pending.kind.summary()), t.muted()),
                ]),
            ]);
        }
        let held = rows.len();
        for budget in &report.budgets {
            let variance = budget.variance();
            let used = match budget.limit {
                Some(limit) if limit > 0.0 => {
                    let frac = budget.actual / limit;
                    self.progress_cell(
                        frac,
                        format!("{:.0}%", frac * 100.0),
                        if frac > 1.0 {
                            Intent::Danger
                        } else if frac > 0.8 {
                            Intent::Warning
                        } else {
                            Intent::Primary
                        },
                    )
                }
                _ => Line::styled("no limit", t.faint()),
            };
            rows.push(vec![
                Line::styled(humanize(&budget.center.as_key()), t.text()),
                Line::styled(
                    budget.limit.map(compact).unwrap_or_else(|| "∞".into()),
                    t.muted(),
                ),
                Line::styled(compact(budget.actual), t.text()),
                Line::styled(compact(budget.forecast), t.text()),
                Line::styled(
                    format!("{variance:+.0}"),
                    t.intent(if variance >= 0.0 {
                        Intent::Success
                    } else {
                        Intent::Danger
                    }),
                ),
                Line::styled(
                    compact(budget.run_rate(report.elapsed_secs, cfg.period_secs)),
                    t.muted(),
                ),
                used,
            ]);
        }
        let mut groups = Vec::new();
        if held > 0 {
            groups.push((0, "held spend".to_string()));
        }
        if rows.len() > held {
            groups.push((held, "budgets".into()));
        }
        ListView {
            strip,
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("center", 14),
                    Column::right("limit", 6),
                    Column::right("actual", 7),
                    Column::right("forecast", 8),
                    Column::right("var", 6),
                    Column::right("run", 6),
                    Column::fill("used"),
                ],
                rows,
                groups,
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }

    fn staff_view(&self) -> ListView {
        let t = &self.theme;
        let now = self.state.last_tick_unix;
        let mut head = vec![
            self.props(&[
                ("leaders", self.state.leaders.len().to_string()),
                (
                    "planet without governor",
                    format!(
                        "−{:.0}% output",
                        self.state.config.leaders.empty_governor * 100.0
                    ),
                ),
            ]),
            Line::styled(
                "governor: scales one planet's output by trait · admiral: leads the home fleet in battle \
                 · minister: holds policy clearance",
                t.muted(),
            ),
        ];
        if self.state.leaders.is_empty() {
            head.push(Line::default());
            head.extend(callout_lines(
                t,
                Intent::Primary,
                "No leaders",
                "Leaders join as the empire grows. Recruit them here, then assign a role.",
            ));
            return ListView {
                head,
                ..ListView::default()
            };
        }
        let rows = self
            .state
            .leaders
            .iter()
            .map(|leader| {
                let assignment = self.state.assignment_of(leader.id);
                let role = assignment.map(|row| row.role.as_key()).unwrap_or("idle");
                let target = assignment
                    .map(|row| row.target.label())
                    .unwrap_or_else(|| "—".into());
                let mut state = vec![match assignment {
                    Some(row) if row.ready(now) => tag(t, "READY", Intent::Success, false),
                    Some(row) => tag(
                        t,
                        &format!("ETA {}", duration(row.ready_at.saturating_sub(now))),
                        Intent::Warning,
                        true,
                    ),
                    None => tag(t, "UNASSIGNED", Intent::Neutral, true),
                }];
                if leader.retired {
                    state.push(Span::raw(" "));
                    state.push(tag(t, "RETIRED", Intent::Neutral, true));
                }
                vec![
                    Line::styled(format!("#{} {}", leader.id, leader.name), t.text()),
                    Line::styled(
                        self.state
                            .config
                            .leaders
                            .trait_by_id(&leader.trait_id)
                            .map_or_else(|| "no trait".into(), trait_effect),
                        t.muted(),
                    ),
                    self.chip(
                        role,
                        if assignment.is_some() {
                            Intent::Primary
                        } else {
                            Intent::Neutral
                        },
                        true,
                    ),
                    Line::styled(target, t.muted()),
                    Line::from(state),
                ]
            })
            .collect();
        ListView {
            head,
            table: Some(TableView {
                columns: vec![
                    Column::left("leader", 20),
                    // Fits the longest shipped trait: `Stalwart -5% attack +10% shields`.
                    Column::left("trait", 32),
                    Column::left("role", 10),
                    Column::left("target", 14),
                    Column::fill("state"),
                ],
                rows,
                groups: Vec::new(),
                selected: Some(self.selected),
            }),
            ..ListView::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::humanize;

    #[test]
    fn humanize_capitalises_and_spaces_keys() {
        assert_eq!(humanize("iron_ore"), "Iron ore");
        assert_eq!(humanize("energy-balance"), "Energy balance");
        assert_eq!(humanize(""), "");
    }
}
