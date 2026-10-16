//! Frame pieces: surfaces, navbar, status bar, tabs, tiles and dialogs.

use super::spans::key_hints;
use super::theme::{Surface, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};

/// Clears `area` and paints it with `style`.
pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_style(style);
        }
    }
}

/// Pushes already drawn content into the background behind a window.
pub fn dim(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let area = area.intersection(buf.area);
    let style = theme.surface(Surface::Dimmed).patch(theme.faint());
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.modifier = Modifier::empty();
            cell.set_style(style);
        }
    }
}

/// Draws `line` at the top-left of `area`, clipped to its width.
pub fn put(buf: &mut Buffer, area: Rect, line: &Line<'_>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    buf.set_line(area.x, area.y, line, area.width);
}

/// Draws `left` from the left edge and `right` flush with the right edge.
/// `right` is dropped when both do not fit.
pub fn put_split(buf: &mut Buffer, area: Rect, left: &Line<'_>, right: &Line<'_>) {
    put(buf, area, left);
    let rw = right.width() as u16;
    if rw == 0 || left.width() as u16 + rw + 1 > area.width {
        return;
    }
    let x = area.right() - rw;
    buf.set_line(x, area.y, right, rw);
}

/// Top bar: app mark, breadcrumbs, then right-aligned spans (search, tags).
pub struct Navbar<'a> {
    pub theme: &'a Theme,
    pub mark: &'a str,
    pub crumbs: Vec<String>,
    pub right: Vec<Span<'static>>,
}

impl Widget for Navbar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        fill(buf, area, t.surface(Surface::Raised));
        let mut left = vec![
            Span::raw(" "),
            Span::styled(
                format!("◆ {}", self.mark),
                t.intent(super::Intent::Primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  ", t.line()),
        ];
        let last = self.crumbs.len().saturating_sub(1);
        for (i, crumb) in self.crumbs.iter().enumerate() {
            if i > 0 {
                left.push(Span::styled(" / ", t.label()));
            }
            let style = if i == last { t.strong() } else { t.muted() };
            left.push(Span::styled(crumb.clone(), style));
        }
        let mut right = self.right;
        right.push(Span::raw(" "));
        put_split(buf, area, &Line::from(left), &Line::from(right));
    }
}

/// Bottom bar: uppercase breadcrumb, keycap hints that fit, and a right
/// side that wins over the hints.
pub struct StatusBar<'a> {
    pub theme: &'a Theme,
    pub crumbs: Vec<String>,
    pub hints: Vec<(String, String)>,
    pub right: Option<Line<'static>>,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        fill(buf, area, t.surface(Surface::Raised));
        let mut right = self.right.unwrap_or_default();
        if right.width() > 0 {
            right.spans.push(Span::raw(" "));
        }
        let room = (area.width as usize).saturating_sub(right.width() + 1);
        let mut left = vec![Span::raw(" ")];
        let last = self.crumbs.len().saturating_sub(1);
        for (i, crumb) in self.crumbs.iter().enumerate() {
            if i > 0 {
                left.push(Span::styled(" › ", t.faint()));
            }
            let style = if i == last { t.muted() } else { t.label() };
            left.push(Span::styled(crumb.to_uppercase(), style));
        }
        left.push(Span::raw("    "));
        let mut used: usize = left.iter().map(|s| s.width()).sum();
        for hint in &self.hints {
            let spans = key_hints(t, std::slice::from_ref(hint));
            let w: usize = spans.iter().map(|s| s.width()).sum::<usize>() + 2;
            if used + w > room {
                break;
            }
            used += w;
            left.extend(spans);
            left.push(Span::raw("  "));
        }
        put(buf, area, &Line::from(left));
        let rw = right.width() as u16;
        if rw > 0 && rw <= area.width {
            buf.set_line(area.right() - rw, area.y, &right, rw);
        }
    }
}

/// Text tabs with an underline under the selected one. Two rows tall.
pub struct Tabs<'a> {
    pub theme: &'a Theme,
    pub items: Vec<String>,
    pub selected: usize,
}

impl Tabs<'_> {
    pub const HEIGHT: u16 = 2;
}

impl Widget for Tabs<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        if area.height == 0 {
            return;
        }
        let mut labels = vec![Span::raw(" ")];
        let mut under = vec![Span::styled("─", t.line())];
        for (i, item) in self.items.iter().enumerate() {
            let w = item.chars().count();
            if i > 0 {
                labels.push(Span::raw("   "));
                under.push(Span::styled("───", t.line()));
            }
            if i == self.selected {
                labels.push(Span::styled(item.clone(), t.strong()));
                let rule = if t.plain { "▀" } else { "━" };
                under.push(Span::styled(
                    rule.repeat(w),
                    t.intent(super::Intent::Primary),
                ));
            } else {
                labels.push(Span::styled(item.clone(), t.muted()));
                under.push(Span::styled("─".repeat(w), t.line()));
            }
        }
        let used: usize = under.iter().map(|s| s.content.chars().count()).sum();
        under.push(Span::styled(
            "─".repeat((area.width as usize).saturating_sub(used)),
            t.line(),
        ));
        put(buf, area, &Line::from(labels));
        if area.height > 1 {
            put(
                buf,
                Rect {
                    y: area.y + 1,
                    height: 1,
                    ..area
                },
                &Line::from(under),
            );
        }
    }
}

/// Bridge tile: uppercase label row on a surface, body below.
pub struct Tile<'a> {
    pub theme: &'a Theme,
    pub title: String,
    pub right: Option<Line<'static>>,
    pub focused: bool,
}

impl Tile<'_> {
    pub fn inner(area: Rect) -> Rect {
        Rect {
            x: area.x + 1.min(area.width),
            y: area.y + 2.min(area.height),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    }
}

impl Widget for Tile<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        fill(buf, area, t.surface(Surface::Raised));
        if t.plain {
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(if self.focused { t.strong() } else { t.line() })
                .render(area, buf);
        }
        let title_style = if self.focused {
            t.intent(super::Intent::Primary)
                .add_modifier(Modifier::BOLD)
        } else {
            t.label()
        };
        let mark = if self.focused { "▌" } else { " " };
        let left = Line::from(vec![
            Span::styled(mark, t.intent(super::Intent::Primary)),
            Span::styled(self.title.to_uppercase(), title_style),
        ]);
        let mut right = self.right.unwrap_or_default();
        right.spans.push(Span::raw(" "));
        put_split(buf, Rect { height: 1, ..area }, &left, &right);
    }
}

/// Window: header bar, raised body, footer bar with right-aligned buttons.
pub struct Dialog<'a> {
    pub theme: &'a Theme,
    pub title: String,
    pub subtitle: String,
    pub close_hint: String,
    pub footer_note: Option<Line<'static>>,
    pub buttons: Vec<Span<'static>>,
}

impl Dialog<'_> {
    /// Body area inside the header and footer bars (and the border in plain mode).
    pub fn inner(&self, area: Rect) -> Rect {
        let side = u16::from(self.theme.plain);
        Rect {
            x: area.x + side.min(area.width),
            y: area.y + 1.min(area.height),
            width: area.width.saturating_sub(2 * side),
            height: area.height.saturating_sub(2),
        }
    }
}

impl Widget for Dialog<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        if area.height < 2 {
            return;
        }
        fill(buf, area, t.surface(Surface::Raised));
        if t.plain {
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(t.strong())
                .render(area, buf);
        }
        let head = Rect { height: 1, ..area };
        let foot = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        if !t.plain {
            fill(buf, head, t.surface(Surface::Header));
            fill(buf, foot, t.surface(Surface::Header));
        }
        let title = Line::from(vec![
            Span::raw(" "),
            Span::styled("▦ ", t.intent(super::Intent::Primary)),
            Span::styled(self.title, t.strong()),
            Span::raw("   "),
            Span::styled(self.subtitle, t.muted()),
        ]);
        let close = Line::from(vec![
            Span::styled(self.close_hint, t.label()),
            Span::raw(" "),
        ]);
        put_split(buf, inset_x(head, u16::from(t.plain)), &title, &close);
        let mut buttons = Vec::new();
        for (i, b) in self.buttons.into_iter().enumerate() {
            if i > 0 {
                buttons.push(Span::raw("  "));
            }
            buttons.push(b);
        }
        buttons.push(Span::raw(" "));
        let note = self.footer_note.unwrap_or_default();
        let mut left = Line::from(Span::raw(" "));
        left.spans.extend(note.spans);
        put_split(
            buf,
            inset_x(foot, u16::from(t.plain)),
            &left,
            &Line::from(buttons),
        );
    }
}

fn inset_x(area: Rect, n: u16) -> Rect {
    Rect {
        x: area.x + n.min(area.width),
        width: area.width.saturating_sub(2 * n),
        ..area
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::spans::button;
    use crate::ui::test_util::row_text;

    #[test]
    fn navbar_puts_crumbs_left_and_status_right() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        Navbar {
            theme: &theme,
            mark: "HOSHI",
            crumbs: vec!["Bridge".into(), "Colony".into()],
            right: vec![Span::raw("RAID 1:00")],
        }
        .render(area, &mut buf);
        let row = row_text(&buf, 0);
        assert!(row.starts_with(" ◆ HOSHI  │  Bridge / Colony"), "{row}");
        assert!(row.ends_with("RAID 1:00 "), "{row:?}");
    }

    #[test]
    fn tabs_underline_only_the_selected_label() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 40, 2);
        let mut buf = Buffer::empty(area);
        Tabs {
            theme: &theme,
            items: vec!["Buildings".into(), "Wonder".into()],
            selected: 1,
        }
        .render(area, &mut buf);
        assert_eq!(row_text(&buf, 0).trim_end(), " Buildings   Wonder");
        let under = row_text(&buf, 1);
        assert_eq!(under.chars().count(), 40);
        assert_eq!(under.chars().skip(13).take(6).collect::<String>(), "━━━━━━");
        assert!(!under.chars().take(13).any(|c| c == '━'), "{under}");
    }

    #[test]
    fn dialog_frames_header_body_and_footer() {
        for theme in [Theme::default(), Theme::high_contrast()] {
            let area = Rect::new(0, 0, 50, 8);
            let mut buf = Buffer::empty(area);
            let dialog = Dialog {
                theme: &theme,
                title: "Colony".into(),
                subtitle: "Home 1:1:1".into(),
                close_hint: "✕ esc".into(),
                footer_note: None,
                buttons: vec![button(&theme, "⏎ Build", true)],
            };
            let inner = dialog.inner(area);
            assert_eq!(inner.y, 1);
            assert_eq!(inner.height, 6);
            dialog.render(area, &mut buf);
            assert!(row_text(&buf, 0).contains("▦ Colony   Home 1:1:1"));
            assert!(row_text(&buf, 0).contains("✕ esc"));
            assert!(row_text(&buf, 7).contains("⏎ Build"));
        }
    }

    #[test]
    fn dim_recolours_without_erasing_text() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        put(&mut buf, area, &Line::styled("alerts", theme.strong()));
        dim(&mut buf, area, &theme);
        assert_eq!(row_text(&buf, 0).trim_end(), "alerts");
        assert_eq!(buf[(0, 0)].fg, theme.faint().fg.unwrap());
        assert!(!buf[(0, 0)].modifier.contains(Modifier::BOLD));
    }
}
