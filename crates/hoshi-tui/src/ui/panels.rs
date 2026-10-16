//! Content blocks: metric strip, property list, callouts, section labels.

use super::chrome::{fill, put};
use super::theme::{Intent, Surface, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Uppercase muted label that opens a block, such as `COST`.
pub fn section(theme: &Theme, title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_uppercase(), theme.label()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub label: String,
    pub value: Line<'static>,
    pub foot: Line<'static>,
}

/// Row of equal metric cells separated by thin rules. Three rows tall.
/// Labels render as given so symbols such as `Fe` keep their case.
pub struct MetricStrip<'a> {
    pub theme: &'a Theme,
    pub metrics: Vec<Metric>,
}

impl MetricStrip<'_> {
    pub const HEIGHT: u16 = 3;

    pub fn cell_width(&self, total: u16) -> u16 {
        let n = self.metrics.len().max(1) as u16;
        total / n
    }
}

impl Widget for MetricStrip<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        fill(buf, area, t.surface(Surface::Base));
        let cw = self.cell_width(area.width);
        if cw < 3 {
            return;
        }
        for (i, metric) in self.metrics.iter().enumerate() {
            let x = area.x + cw * i as u16;
            if i > 0 {
                for dy in 0..area.height.min(Self::HEIGHT) {
                    buf[(x, area.y + dy)].set_symbol("│").set_style(t.line());
                }
            }
            let last = i + 1 == self.metrics.len();
            let w = if last {
                area.right().saturating_sub(x + 2)
            } else {
                cw.saturating_sub(2)
            };
            let cell = Rect::new(x + 2, area.y, w, area.height);
            let label = Line::styled(metric.label.clone(), t.label());
            for (dy, line) in [&label, &metric.value, &metric.foot]
                .into_iter()
                .enumerate()
            {
                if (dy as u16) < cell.height {
                    put(
                        buf,
                        Rect {
                            y: cell.y + dy as u16,
                            height: 1,
                            ..cell
                        },
                        line,
                    );
                }
            }
        }
    }
}

/// Key-value grid for the inspector. Labels share one column width.
pub struct PropertyList<'a> {
    pub theme: &'a Theme,
    pub rows: Vec<(String, Line<'static>)>,
}

impl PropertyList<'_> {
    pub fn lines(self) -> Vec<Line<'static>> {
        let w = self
            .rows
            .iter()
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        self.rows
            .into_iter()
            .map(|(k, v)| {
                let mut spans = vec![Span::styled(format!("{k:<w$}"), self.theme.label())];
                spans.extend(v.spans);
                Line::from(spans)
            })
            .collect()
    }
}

/// Alert with an intent-coloured left bar, a title and an optional detail line.
pub struct Callout<'a> {
    pub theme: &'a Theme,
    pub intent: Intent,
    pub title: Line<'static>,
    pub detail: Option<Line<'static>>,
}

impl Callout<'_> {
    pub fn lines(self) -> Vec<Line<'static>> {
        let bar = Span::styled("▌ ", self.theme.intent(self.intent));
        let mut first = vec![bar];
        first.extend(self.title.spans.into_iter().map(|s| {
            if s.style == Default::default() {
                s.style(self.theme.intent(self.intent))
            } else {
                s
            }
        }));
        let mut out = vec![Line::from(first)];
        if let Some(detail) = self.detail {
            let mut second = vec![Span::raw("  ")];
            second.extend(detail.spans.into_iter().map(|s| {
                if s.style == Default::default() {
                    s.style(self.theme.muted())
                } else {
                    s
                }
            }));
            out.push(Line::from(second));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_util::row_text;

    #[test]
    fn metric_strip_splits_cells_with_rules() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        MetricStrip {
            theme: &theme,
            metrics: vec![
                Metric {
                    label: "Fe".into(),
                    value: Line::from("500 / 10k"),
                    foot: Line::from("+33/h"),
                },
                Metric {
                    label: "Heat".into(),
                    value: Line::from("22 / 80"),
                    foot: Line::from("28%"),
                },
            ],
        }
        .render(area, &mut buf);
        assert_eq!(
            row_text(&buf, 0).trim_end(),
            format!("  Fe{}│ Heat", " ".repeat(16))
        );
        assert_eq!(buf[(20, 1)].symbol(), "│");
        assert!(row_text(&buf, 1).contains("500 / 10k"));
        assert!(row_text(&buf, 2).contains("28%"));
    }

    #[test]
    fn property_labels_share_a_column() {
        let theme = Theme::default();
        let lines = PropertyList {
            theme: &theme,
            rows: vec![
                ("Planet".into(), Line::from("1:1:1")),
                ("Build time".into(), Line::from("3m 00s")),
            ],
        }
        .lines();
        assert_eq!(lines[0].to_string(), "Planet      1:1:1");
        assert_eq!(lines[1].to_string(), "Build time  3m 00s");
    }

    #[test]
    fn callout_colours_plain_title_with_its_intent() {
        let theme = Theme::default();
        let lines = Callout {
            theme: &theme,
            intent: Intent::Danger,
            title: Line::from("Incoming raid"),
            detail: Some(Line::from("pirates → 1:1:1")),
        }
        .lines();
        assert_eq!(lines[0].to_string(), "▌ Incoming raid");
        assert_eq!(lines[0].spans[1].style, theme.intent(Intent::Danger));
        assert_eq!(lines[1].to_string(), "  pirates → 1:1:1");
    }
}
