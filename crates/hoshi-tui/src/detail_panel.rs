//! Formats hoshi-core `Detail` values. No game maths live here.

use crate::ui::panels::{Callout, PropertyList, section};
use crate::ui::spans::{bar, compact, duration, tag};
use crate::ui::{Intent, Theme};
use hoshi_core::{AffordEta, Detail};
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
        rows.push(format!("energy {draw:+.0}"));
    }
    if let Some(heat) = detail.heat {
        rows.push(format!("heat {heat:.0}"));
    }
    if let Some(secs) = detail.duration_secs {
        rows.push(format!("time {secs}s"));
    }
    if let Some(eta) = detail.afford {
        rows.push(match eta {
            AffordEta::Now => "affordable now".into(),
            AffordEta::In(secs) => format!("affordable in {secs}s"),
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
        props.push(("Build time".into(), Line::styled(duration(secs), t.text())));
    }
    if let Some(draw) = detail.energy_draw {
        props.push((
            "Energy".into(),
            Line::styled(format!("{draw:+.0}"), t.text()),
        ));
    }
    if let Some(heat) = detail.heat {
        props.push(("Heat".into(), Line::styled(format!("{heat:.0}"), t.text())));
    }
    if let Some(eta) = detail.afford {
        let chip = match eta {
            AffordEta::Now => tag(t, "READY", Intent::Success, false),
            AffordEta::In(secs) => tag(
                t,
                &format!("IN {}", duration(secs).to_uppercase()),
                Intent::Warning,
                true,
            ),
            AffordEta::Never => tag(t, "NEVER AT CURRENT INCOME", Intent::Danger, true),
        };
        props.push(("Status".into(), Line::from(chip)));
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
        let w = costs.iter().map(|row| row.id.len()).max().unwrap_or(0);
        for row in costs {
            let short = row.missing > 0.0;
            let intent = if short {
                Intent::Danger
            } else {
                Intent::Primary
            };
            let mut spans = vec![Span::styled(format!("{:<w$}  ", row.id), t.muted())];
            spans.extend(bar(t, row.cost / row.stock.max(1.0), 16, intent));
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
