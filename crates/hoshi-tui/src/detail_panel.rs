//! Formats hoshi-core `Detail` values. No game maths live here.

use hoshi_core::{AffordEta, Detail};
use ratatui::text::Line;

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
