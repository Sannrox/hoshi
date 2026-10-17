//! Dense data table: uppercase header, striped rows, full-width selection.

use super::chrome::fill;
use super::theme::{Surface, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub title: String,
    /// Cell width; `0` takes whatever is left (at most one such column).
    pub width: u16,
    pub align: Align,
}

impl Column {
    pub fn left(title: &str, width: u16) -> Self {
        Column {
            title: title.into(),
            width,
            align: Align::Left,
        }
    }

    pub fn right(title: &str, width: u16) -> Self {
        Column {
            title: title.into(),
            width,
            align: Align::Right,
        }
    }

    pub fn fill(title: &str) -> Self {
        Column::left(title, 0)
    }
}

pub struct DataTable<'a> {
    pub theme: &'a Theme,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Line<'static>>>,
    pub selected: Option<usize>,
    pub striped: bool,
    pub surface: Surface,
    /// Label rows drawn before the data row at each index, such as `SHIPS`.
    pub groups: Vec<(usize, String)>,
}

impl DataTable<'_> {
    const GAP: u16 = 2;

    fn widths(&self, total: u16) -> Vec<u16> {
        let fixed: u16 = self
            .columns
            .iter()
            .map(|c| c.width)
            .sum::<u16>()
            .saturating_add(Self::GAP * self.columns.len().saturating_sub(1) as u16)
            .saturating_add(1);
        let rest = total.saturating_sub(fixed);
        self.columns
            .iter()
            .map(|c| if c.width == 0 { rest } else { c.width })
            .collect()
    }

    /// First visible row so the selection stays on screen, roughly centred.
    pub fn scroll(selected: Option<usize>, rows: usize, visible: usize) -> usize {
        let Some(sel) = selected else { return 0 };
        if visible == 0 || rows <= visible {
            return 0;
        }
        sel.saturating_sub(visible / 2).min(rows - visible)
    }
}

impl Widget for DataTable<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        if area.height == 0 || area.width == 0 {
            return;
        }
        fill(buf, area, t.surface(self.surface));
        let widths = self.widths(area.width);
        let header: Vec<Line<'static>> = self
            .columns
            .iter()
            .map(|c| Line::styled(c.title.to_uppercase(), t.label()))
            .collect();
        draw_row(
            buf,
            area.x,
            area.y,
            area.width,
            &self.columns,
            &widths,
            &header,
        );
        enum Entry<'a> {
            Group(&'a str),
            Row(usize),
        }
        let mut entries = Vec::with_capacity(self.rows.len() + self.groups.len());
        for i in 0..self.rows.len() {
            for (_, label) in self.groups.iter().filter(|(at, _)| *at == i) {
                entries.push(Entry::Group(label));
            }
            entries.push(Entry::Row(i));
        }
        let selected_line = self.selected.and_then(|sel| {
            entries
                .iter()
                .position(|e| matches!(e, Entry::Row(i) if *i == sel))
        });
        let visible = (area.height - 1) as usize;
        let start = Self::scroll(selected_line, entries.len(), visible);
        for (n, entry) in entries.iter().enumerate().skip(start).take(visible) {
            let y = area.y + 1 + (n - start) as u16;
            let band = Rect::new(area.x, y, area.width, 1);
            let i = match entry {
                Entry::Group(label) => {
                    buf.set_line(
                        area.x + 1,
                        y,
                        &Line::styled(label.to_uppercase(), t.label()),
                        area.width.saturating_sub(1),
                    );
                    continue;
                }
                Entry::Row(i) => *i,
            };
            let row = &self.rows[i];
            if Some(i) == self.selected {
                fill(buf, band, t.surface(self.surface).patch(t.selected()));
            } else if self.striped && i % 2 == 1 {
                fill(buf, band, t.surface(Surface::Stripe));
            }
            draw_row(buf, area.x, y, area.width, &self.columns, &widths, row);
            if Some(i) == self.selected && !t.plain {
                buf[(area.x, y)]
                    .set_symbol("▌")
                    .set_style(t.intent(super::Intent::Primary));
            }
        }
    }
}

fn draw_row(
    buf: &mut Buffer,
    x0: u16,
    y: u16,
    total: u16,
    columns: &[Column],
    widths: &[u16],
    cells: &[Line<'_>],
) {
    let mut x = x0 + 1;
    let end = x0 + total;
    for (i, col) in columns.iter().enumerate() {
        let w = widths[i].min(end.saturating_sub(x));
        if w == 0 {
            break;
        }
        if let Some(cell) = cells.get(i) {
            let cw = (cell.width() as u16).min(w);
            let cx = match col.align {
                Align::Left => x,
                Align::Right => x + w - cw,
            };
            buf.set_line(cx, y, cell, w - (cx - x));
        }
        x = x.saturating_add(w + DataTable::GAP);
        if x >= end {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_util::row_text;
    use ratatui::style::Modifier;

    fn table(theme: &Theme, selected: Option<usize>, n: usize) -> DataTable<'_> {
        DataTable {
            theme,
            columns: vec![
                Column::left("name", 8),
                Column::right("fe", 5),
                Column::fill("status"),
            ],
            rows: (0..n)
                .map(|i| {
                    vec![
                        Line::from(format!("row{i}")),
                        Line::from(format!("{}", i * 10)),
                        Line::from("ok"),
                    ]
                })
                .collect(),
            selected,
            striped: true,
            surface: Surface::Raised,
            groups: Vec::new(),
        }
    }

    #[test]
    fn header_is_uppercase_and_numbers_align_right() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        table(&theme, None, 3).render(area, &mut buf);
        assert_eq!(row_text(&buf, 0).trim_end(), " NAME         FE  STATUS");
        assert_eq!(row_text(&buf, 2).trim_end(), " row1         10  ok");
    }

    #[test]
    fn selection_scrolls_into_view_and_is_highlighted() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        table(&theme, Some(9), 10).render(area, &mut buf);
        let last = row_text(&buf, 3);
        assert!(last.contains("row9"), "{last}");
        assert_eq!(buf[(5, 3)].bg, theme.selected().bg.unwrap());
        assert_eq!(buf[(0, 3)].symbol(), "▌");
    }

    #[test]
    fn plain_theme_reverses_the_selected_row() {
        let theme = Theme::high_contrast();
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        table(&theme, Some(0), 2).render(area, &mut buf);
        assert!(buf[(3, 1)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn group_labels_sit_above_their_rows() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let mut t = table(&theme, Some(2), 3);
        t.groups = vec![(0, "ships".into()), (2, "defences".into())];
        t.render(area, &mut buf);
        assert_eq!(row_text(&buf, 1).trim_end(), " SHIPS");
        assert!(row_text(&buf, 2).contains("row0"));
        assert_eq!(row_text(&buf, 4).trim_end(), " DEFENCES");
        assert!(row_text(&buf, 5).contains("row2"));
        assert_eq!(buf[(0, 5)].symbol(), "▌");
    }

    #[test]
    fn scroll_keeps_selection_visible() {
        assert_eq!(DataTable::scroll(Some(0), 50, 10), 0);
        assert_eq!(DataTable::scroll(Some(25), 50, 10), 20);
        assert_eq!(DataTable::scroll(Some(49), 50, 10), 40);
        assert_eq!(DataTable::scroll(Some(3), 5, 10), 0);
    }
}
