//! Inline pieces that sit inside a line: tags, keycaps, buttons, bars.

use super::theme::{Intent, Theme};
use ratatui::text::Span;

const EIGHTHS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// Status chip such as `READY` or `D 200`. Minimal tags are tinted, not filled.
pub fn tag(theme: &Theme, text: &str, intent: Intent, minimal: bool) -> Span<'static> {
    Span::styled(format!(" {text} "), theme.tag(intent, minimal))
}

pub fn keycap(theme: &Theme, key: &str) -> Span<'static> {
    Span::styled(format!(" {key} "), theme.key())
}

pub fn button(theme: &Theme, label: &str, primary: bool) -> Span<'static> {
    Span::styled(format!(" {label} "), theme.button(primary))
}

/// `keycap label  keycap label` hints for footers and status bars.
pub fn key_hints(theme: &Theme, hints: &[(String, String)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(keycap(theme, key));
        spans.push(Span::styled(format!(" {label}"), theme.muted()));
    }
    spans
}

/// Progress bar `width` cells wide with eighth-block precision. Any non-zero
/// fraction shows at least one sliver so "almost empty" differs from empty.
pub fn bar(theme: &Theme, frac: f64, width: usize, intent: Intent) -> Vec<Span<'static>> {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut units = (frac * width as f64 * 8.0).round() as usize;
    if frac > 0.0 && units == 0 {
        units = 1;
    }
    let full = (units / 8).min(width);
    let part = if full < width { units % 8 } else { 0 };
    let mut filled = "█".repeat(full);
    filled.push_str(EIGHTHS[part]);
    let used = full + usize::from(part > 0);
    vec![
        Span::styled(filled, theme.intent(intent)),
        Span::styled("─".repeat(width - used), theme.line()),
    ]
}

/// Block sparkline of the last `width` values, scaled between their min and max.
pub fn sparkline(theme: &Theme, values: &[f64], width: usize, intent: Intent) -> Span<'static> {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let tail = &values[values.len().saturating_sub(width)..];
    let (lo, hi) = tail
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        });
    let span = if hi > lo { hi - lo } else { 1.0 };
    let text: String = tail
        .iter()
        .map(|v| BLOCKS[(((v - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize])
        .collect();
    Span::styled(text, theme.intent(intent))
}

/// Compact duration: `45s`, `3m 00s`, `2h 05m`, `3d 04h`.
pub fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m {:02}s", s / 60, s % 60),
        s if s < 86_400 => format!("{}h {:02}m", s / 3_600, (s % 3_600) / 60),
        s => format!("{}d {:02}h", s / 86_400, (s % 86_400) / 3_600),
    }
}

/// Clock-style countdown for timers: `59:42`, `1:05:00`.
pub fn countdown(secs: u64) -> String {
    if secs >= 3_600 {
        format!(
            "{}:{:02}:{:02}",
            secs / 3_600,
            (secs % 3_600) / 60,
            secs % 60
        )
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// `+500`, `−388`, `0`: [`compact`] with an explicit sign and a true minus.
pub fn signed(v: f64) -> String {
    let text = compact(v.abs());
    match text.as_str() {
        "0" => text,
        _ if v < 0.0 => format!("−{text}"),
        _ => format!("+{text}"),
    }
}

/// `500`, `12.5k`, `3.2M` for tight cells.
pub fn compact(v: f64) -> String {
    let a = v.abs();
    if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}k", v / 1e3)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn bar_keeps_its_width_and_shows_a_sliver() {
        let theme = Theme::default();
        for frac in [0.0, 0.001, 0.18, 0.5, 0.99, 1.0, 7.0, f64::NAN] {
            let s = text(&bar(&theme, frac, 12, Intent::Success));
            assert_eq!(s.chars().count(), 12, "{frac}: {s}");
        }
        assert!(text(&bar(&theme, 0.001, 12, Intent::Success)).starts_with('▏'));
        assert_eq!(
            text(&bar(&theme, 1.0, 4, Intent::Success)),
            "████",
            "full bar"
        );
        assert_eq!(text(&bar(&theme, 0.0, 4, Intent::Success)), "────");
    }

    #[test]
    fn durations_and_countdowns_read_naturally() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(180), "3m 00s");
        assert_eq!(duration(7_500), "2h 05m");
        assert_eq!(duration(273_600), "3d 04h");
        assert_eq!(countdown(3_582), "59:42");
        assert_eq!(countdown(3_900), "1:05:00");
    }

    #[test]
    fn sparkline_scales_to_the_range_and_keeps_the_tail() {
        let theme = Theme::default();
        let s = sparkline(&theme, &[9.0, 1.0, 2.0, 3.0], 3, Intent::Primary);
        assert_eq!(s.content, "▁▅█");
        assert_eq!(sparkline(&theme, &[], 4, Intent::Primary).content, "");
        assert_eq!(
            sparkline(&theme, &[5.0, 5.0], 4, Intent::Primary).content,
            "▁▁"
        );
    }

    #[test]
    fn compact_numbers_fit_five_cells() {
        assert_eq!(compact(500.0), "500");
        assert_eq!(compact(1_260.0), "1.3k");
        assert_eq!(compact(45_000.0), "45k");
        assert_eq!(compact(3_200_000.0), "3.2M");
    }

    #[test]
    fn hints_pair_keycaps_with_labels() {
        let theme = Theme::default();
        let spans = key_hints(
            &theme,
            &[("tab".into(), "tabs".into()), ("⏎".into(), "build".into())],
        );
        assert_eq!(text(&spans), " tab  tabs   ⏎  build");
    }
}
