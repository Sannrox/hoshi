//! Formats hoshi-core `Detail` values. No game maths live here.

use crate::ui::panels::{Callout, PropertyList, section};
use crate::ui::spans::{bar, compact, duration};
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

/// Inspector card: title, properties, cost bars, effects and requirements.
pub fn inspector(detail: &Detail, t: &Theme) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::styled(detail.name.clone(), t.strong()),
        Line::styled(detail.description.clone(), t.muted()),
        Line::default(),
    ];
    let mut props = vec![(
        "level".to_string(),
        Line::styled(detail.level.to_string(), t.text()),
    )];
    if let Some(secs) = detail.duration_secs {
        props.push(("time".into(), Line::styled(duration(secs), t.text())));
    }
    if let Some(draw) = detail.energy_draw {
        props.push((
            "energy".into(),
            Line::styled(format!("{draw:+.0}"), t.text()),
        ));
    }
    if let Some(heat) = detail.heat {
        props.push(("heat".into(), Line::styled(format!("{heat:.0}"), t.text())));
    }
    out.extend(
        PropertyList {
            theme: t,
            rows: props,
        }
        .lines(),
    );
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
                Intent::Warning
            } else {
                Intent::Success
            };
            let mut spans = vec![Span::styled(format!("{:<w$}  ", row.id), t.label())];
            spans.extend(bar(t, row.stock / row.cost.max(1.0), 10, intent));
            spans.push(Span::styled(
                format!("  {} / {}", compact(row.stock), compact(row.cost)),
                t.text(),
            ));
            if short {
                spans.push(Span::styled(
                    format!("  -{}", compact(row.missing)),
                    t.intent(Intent::Warning),
                ));
            }
            out.push(Line::from(spans));
        }
    }
    if let Some(eta) = detail.afford {
        let (intent, title) = match eta {
            AffordEta::Now => (Intent::Success, "affordable now".to_string()),
            AffordEta::In(secs) => (Intent::Warning, format!("affordable in {}", duration(secs))),
            AffordEta::Never => (Intent::Danger, "never at current income".to_string()),
        };
        out.push(Line::default());
        out.extend(
            Callout {
                theme: t,
                intent,
                title: Line::from(title),
                detail: None,
            }
            .lines(),
        );
    }
    if !detail.deltas.is_empty() {
        out.push(Line::default());
        out.push(section(t, "effect"));
        let rows = detail
            .deltas
            .iter()
            .map(|d| {
                (
                    d.label.clone(),
                    Line::from(vec![
                        Span::styled(d.from.clone(), t.muted()),
                        Span::styled(" → ", t.label()),
                        Span::styled(d.to.clone(), t.intent(Intent::Success)),
                    ]),
                )
            })
            .collect();
        out.extend(PropertyList { theme: t, rows }.lines());
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
