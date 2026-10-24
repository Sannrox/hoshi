//! Formats hoshi-core `Detail` values. No game maths live here.

use crate::ui::panels::{Callout, PropertyList, section};
use crate::ui::spans::{bar, compact, duration, tag};
use crate::ui::{Intent, Theme};
use hoshi_core::{AffordEta, Detail, ResourceKind};
use ratatui::text::{Line, Span};

/// Dock the side card when the frame is at least this wide.
pub const DOCK_MIN_WIDTH: u16 = 120;

pub fn lines(detail: &Detail) -> Vec<String> {
    let mut rows = vec![
        detail.name.clone(),
        detail.description.clone(),
        format!("level {}", detail.level),
    ];
    if !detail.costs.is_empty() {
        rows.push("cost".into());
        for row in &detail.costs {
            if row.cost <= 0.0 && row.missing <= 0.0 {
                continue;
            }
            rows.push(format!(
                "  {}  stock {:.0}  cost {:.0}  missing {:.0}",
                row.id, row.stock, row.cost, row.missing
            ));
        }
    }
    if let Some(draw) = detail.energy_draw {
        rows.push(format!("energy {}", energy_text(draw)));
    }
    if let Some(heat) = detail.heat {
        rows.push(format!("heat {heat:.0}"));
    }
    if let Some(secs) = detail.duration_secs {
        rows.push(format!("time {}", duration(secs)));
    }
    if let Some(eta) = detail.afford {
        rows.push(match eta {
            AffordEta::Now => "affordable now".into(),
            AffordEta::In(secs) => format!("affordable in {}", duration(secs)),
            AffordEta::Never => "never at current income".into(),
        });
    }
    for delta in &detail.deltas {
        rows.push(format!("{}  {} -> {}", delta.label, delta.from, delta.to));
    }
    if !detail.requirements.is_empty() {
        rows.push("requires".into());
        for req in &detail.requirements {
            let mark = if req.met { "ok" } else { "need" };
            rows.push(format!("  {mark} {}", req.text));
        }
    }
    if !detail.unlocks.is_empty() {
        rows.push(format!("unlocks {}", detail.unlocks.join(", ")));
    }
    if let Some(warning) = &detail.exclusive_warning {
        rows.push(warning.clone());
    }
    if let Some(ship) = &detail.ship {
        rows.push(format!(
            "speed {:.0}  fuel {:.1}/u  cargo {:.0}",
            ship.speed, ship.fuel_per_unit, ship.cargo
        ));
        if let (Some(secs), Some(fuel)) = (ship.outbound_secs, ship.fuel_cost) {
            rows.push(format!("travel {secs}s  fuel {fuel:.1}"));
        }
    }
    rows
}

/// Extra energy an upgrade draws, worded so a cost never reads as output.
fn energy_text(draw: f64) -> String {
    if draw > 0.0 {
        format!("draws {draw:.0} more")
    } else if draw < 0.0 {
        format!("frees {:.0}", -draw)
    } else {
        "no change".into()
    }
}

pub fn paragraph(detail: &Detail) -> Vec<Line<'static>> {
    lines(detail).into_iter().map(Line::from).collect()
}

/// Inspector column: kind label, name with level tag, properties, cost bars,
/// an effect table and requirements.
pub fn inspector(
    detail: &Detail,
    kind: &str,
    levelled: bool,
    extra: Vec<(String, Line<'static>)>,
    t: &Theme,
) -> Vec<Line<'static>> {
    let mut title = vec![Span::styled(detail.name.clone(), t.strong())];
    if levelled {
        title.push(Span::raw("  "));
        title.push(tag(
            t,
            &format!("LV {} → {}", detail.level, detail.level + 1),
            Intent::Primary,
            true,
        ));
    }
    let mut out = vec![
        section(t, kind),
        Line::from(title),
        Line::styled(detail.description.clone(), t.muted()),
        Line::default(),
    ];
    let mut props = extra;
    if let Some(secs) = detail.duration_secs {
        props.push(("Time".into(), Line::styled(duration(secs), t.text())));
    }
    if let Some(draw) = detail.energy_draw {
        props.push(("Energy".into(), Line::styled(energy_text(draw), t.text())));
    }
    if let Some(heat) = detail.heat {
        props.push(("Heat".into(), Line::styled(format!("{heat:.0}"), t.text())));
    }
    // An unmet requirement blocks it whatever the stock, so it outranks the ETA.
    let blocked = detail.requirements.iter().any(|r| !r.met);
    if blocked {
        props.push((
            "Status".into(),
            Line::from(tag(t, "BLOCKED", Intent::Danger, true)),
        ));
    } else if let Some(eta) = detail.afford {
        let chip = match eta {
            AffordEta::Now => tag(t, "READY", Intent::Success, false),
            AffordEta::In(secs) => tag(
                t,
                &format!("IN {}", duration(secs).to_uppercase()),
                Intent::Warning,
                true,
            ),
            AffordEta::Never => tag(t, "NEVER", Intent::Danger, true),
        };
        let mut status = vec![chip];
        if matches!(eta, AffordEta::Never) {
            status.push(Span::styled("at current income", t.muted()));
        }
        props.push(("Status".into(), Line::from(status)));
    }
    if !props.is_empty() {
        out.extend(
            PropertyList {
                theme: t,
                rows: props,
            }
            .lines(),
        );
    }
    let costs: Vec<_> = detail
        .costs
        .iter()
        .filter(|row| row.cost > 0.0 || row.missing > 0.0)
        .collect();
    if !costs.is_empty() {
        out.push(Line::default());
        out.push(section(t, "cost"));
        let name = |id: &str| {
            ResourceKind::ALL
                .iter()
                .find(|kind| kind.as_key() == id)
                .map_or_else(|| id.replace('_', " "), |kind| kind.abbr().to_string())
        };
        let w = costs
            .iter()
            .map(|row| name(row.id).len())
            .max()
            .unwrap_or(0);
        for row in costs {
            let short = row.missing > 0.0;
            let intent = if short {
                Intent::Danger
            } else {
                Intent::Primary
            };
            let mut spans = vec![Span::styled(format!("{:<w$}  ", name(row.id)), t.muted())];
            // Fills toward affording it: full means the stock covers the cost.
            let covered = if row.cost > 0.0 {
                (row.stock / row.cost).min(1.0)
            } else {
                1.0
            };
            spans.extend(bar(t, covered, 10, intent));
            spans.push(Span::styled(
                format!("  {}", compact(row.cost)),
                if short {
                    t.intent(Intent::Danger)
                } else {
                    t.text()
                },
            ));
            spans.push(Span::styled(
                format!(" / {}", compact(row.stock)),
                t.label(),
            ));
            out.push(Line::from(spans));
        }
    }
    if !detail.deltas.is_empty() {
        out.push(Line::default());
        let w = detail
            .deltas
            .iter()
            .map(|d| d.label.chars().count())
            .max()
            .unwrap_or(0)
            .max(6);
        out.push(Line::styled(
            format!("{:<w$} {:>6} {:>6} {:>6}", "EFFECT", "NOW", "NEXT", "Δ"),
            t.label(),
        ));
        for (i, d) in detail.deltas.iter().enumerate() {
            let change = match (d.from.parse::<f64>(), d.to.parse::<f64>()) {
                (Ok(a), Ok(b)) if (b - a).abs() > f64::EPSILON => {
                    let diff = b - a;
                    let text = if diff.fract().abs() < 1e-9 {
                        format!("{diff:+.0}")
                    } else {
                        format!("{diff:+.1}")
                    };
                    Span::styled(format!("{text:>6}"), t.intent(Intent::Success))
                }
                _ => Span::styled(format!("{:>6}", "—"), t.faint()),
            };
            let mut line = Line::from(vec![
                Span::styled(format!("{:<w$} ", d.label), t.muted()),
                Span::styled(format!("{:>6} ", d.from), t.text()),
                Span::styled(format!("{:>6} ", d.to), t.strong()),
                change,
            ]);
            if i % 2 == 0 {
                line = line.style(t.surface(crate::ui::Surface::Stripe));
            }
            out.push(line);
        }
    }
    if !detail.requirements.is_empty() {
        out.push(Line::default());
        out.push(section(t, "requires"));
        for req in &detail.requirements {
            let (mark, intent) = if req.met {
                ("✓", Intent::Success)
            } else {
                ("✗", Intent::Danger)
            };
            out.push(Line::from(vec![
                Span::styled(format!("{mark} "), t.intent(intent)),
                Span::styled(req.text.clone(), if req.met { t.muted() } else { t.text() }),
            ]));
        }
    }
    if !detail.unlocks.is_empty() {
        out.push(Line::default());
        out.push(section(t, "unlocks"));
        out.push(Line::styled(detail.unlocks.join(", "), t.text()));
    }
    if let Some(warning) = &detail.exclusive_warning {
        out.push(Line::default());
        out.extend(
            Callout {
                theme: t,
                intent: Intent::Warning,
                title: Line::from(warning.clone()),
                detail: None,
            }
            .lines(),
        );
    }
    if let Some(ship) = &detail.ship {
        out.push(Line::default());
        out.push(section(t, "travel"));
        let mut rows = vec![
            (
                "speed".to_string(),
                Line::from(format!("{:.0}", ship.speed)),
            ),
            (
                "fuel".to_string(),
                Line::from(format!("{:.1}/u", ship.fuel_per_unit)),
            ),
            (
                "cargo".to_string(),
                Line::from(format!("{:.0}", ship.cargo)),
            ),
        ];
        if let (Some(secs), Some(fuel)) = (ship.outbound_secs, ship.fuel_cost) {
            rows.push((
                "trip".into(),
                Line::from(format!("{}  fuel {fuel:.1}", duration(secs))),
            ));
        }
        out.extend(PropertyList { theme: t, rows }.lines());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{AffordEta, MaterialLine, RequirementLine};

    fn detail(stock: f64, requirement_met: bool) -> Detail {
        Detail {
            name: "Robotics factory".into(),
            description: String::new(),
            level: 0,
            costs: vec![MaterialLine {
                id: "deuterium",
                stock,
                cost: 600.0,
                missing: (600.0 - stock).max(0.0),
            }],
            energy_draw: None,
            heat: None,
            duration_secs: None,
            afford: Some(AffordEta::In(7_200)),
            deltas: Vec::new(),
            requirements: vec![RequirementLine {
                text: "shipyard 2 (have 0)".into(),
                met: requirement_met,
            }],
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        }
    }

    fn text(detail: &Detail) -> String {
        inspector(detail, "building", false, Vec::new(), &Theme::default())
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cost_bar_fills_toward_affording_it() {
        let broke = text(&detail(0.0, true));
        let rich = text(&detail(900.0, true));
        let bar_of = |text: &str| {
            text.lines()
                .find(|line| line.contains("600 /"))
                .unwrap()
                .chars()
                .filter(|c| *c == '█')
                .count()
        };
        assert_eq!(bar_of(&broke), 0, "{broke}");
        assert_eq!(bar_of(&rich), 10, "{rich}");
    }

    #[test]
    fn an_unmet_requirement_outranks_the_affordability_eta() {
        assert!(text(&detail(0.0, false)).contains("BLOCKED"));
        let waiting = text(&detail(0.0, true));
        assert!(waiting.contains("IN 2H 00M"), "{waiting}");
    }
}
