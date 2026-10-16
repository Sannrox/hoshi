//! Slate colour roles. Widgets ask the theme for a role, never for a raw colour.

use ratatui::style::{Color, Modifier, Style};

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

impl Depth {
    /// Reads `NO_COLOR`, `COLORTERM` and `TERM` through `env`. An empty
    /// `NO_COLOR` does not count (no-color.org).
    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Self {
        if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Depth::Mono;
        }
        let colorterm = env("COLORTERM").unwrap_or_default().to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return Depth::TrueColor;
        }
        let term = env("TERM").unwrap_or_default();
        if term == "dumb" {
            Depth::Mono
        } else if term.contains("256color") {
            Depth::Ansi256
        } else {
            Depth::Ansi16
        }
    }
}

/// What a coloured element means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Primary,
    Success,
    Warning,
    Danger,
}

/// Background layers, darkest last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Base,
    Raised,
    Header,
    Stripe,
    Dimmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Palette {
    base: Color,
    raised: Color,
    header: Color,
    stripe: Color,
    dimmed: Color,
    line: Color,
    text: Color,
    muted: Color,
    label: Color,
    faint: Color,
    primary: Color,
    primary_fill: Color,
    primary_soft: Color,
    primary_soft_text: Color,
    selection: Color,
    success: Color,
    success_fill: Color,
    warning: Color,
    warning_soft: Color,
    warning_soft_text: Color,
    danger: Color,
    danger_fill: Color,
    danger_soft: Color,
    danger_soft_text: Color,
    key: Color,
}

type Rgb = (u8, u8, u8);

const SLATE: [Rgb; 25] = [
    (0x1c, 0x21, 0x27), // base
    (0x25, 0x2a, 0x31), // raised
    (0x2f, 0x34, 0x3c), // header
    (0x2a, 0x2f, 0x37), // stripe
    (0x15, 0x18, 0x1d), // dimmed
    (0x40, 0x48, 0x54), // line
    (0xf6, 0xf7, 0xf9), // text
    (0xab, 0xb3, 0xbf), // muted
    (0x73, 0x80, 0x91), // label
    (0x3a, 0x41, 0x4b), // faint
    (0x4c, 0x90, 0xf0), // primary
    (0x2d, 0x72, 0xd2), // primary fill
    (0x1f, 0x33, 0x52), // primary soft
    (0x8a, 0xbb, 0xff), // primary soft text
    (0x1f, 0x3b, 0x5e), // selection
    (0x32, 0xa4, 0x67), // success
    (0x23, 0x85, 0x51), // success fill
    (0xec, 0x9a, 0x3c), // warning
    (0x3b, 0x30, 0x24), // warning soft
    (0xfb, 0xb3, 0x60), // warning soft text
    (0xe7, 0x6a, 0x6e), // danger
    (0xcd, 0x42, 0x46), // danger fill
    (0x43, 0x28, 0x2c), // danger soft
    (0xfa, 0x99, 0x9c), // danger soft text
    (0x38, 0x3e, 0x47), // key
];

impl Palette {
    fn from_rgb(map: impl Fn(Rgb) -> Color) -> Self {
        let c = |i: usize| map(SLATE[i]);
        Palette {
            base: c(0),
            raised: c(1),
            header: c(2),
            stripe: c(3),
            dimmed: c(4),
            line: c(5),
            text: c(6),
            muted: c(7),
            label: c(8),
            faint: c(9),
            primary: c(10),
            primary_fill: c(11),
            primary_soft: c(12),
            primary_soft_text: c(13),
            selection: c(14),
            success: c(15),
            success_fill: c(16),
            warning: c(17),
            warning_soft: c(18),
            warning_soft_text: c(19),
            danger: c(20),
            danger_fill: c(21),
            danger_soft: c(22),
            danger_soft_text: c(23),
            key: c(24),
        }
    }

    fn ansi16() -> Self {
        Palette {
            base: Color::Black,
            raised: Color::Black,
            header: Color::DarkGray,
            stripe: Color::Black,
            dimmed: Color::Black,
            line: Color::DarkGray,
            text: Color::White,
            muted: Color::Gray,
            label: Color::DarkGray,
            faint: Color::DarkGray,
            primary: Color::LightBlue,
            primary_fill: Color::Blue,
            primary_soft: Color::Black,
            primary_soft_text: Color::LightBlue,
            selection: Color::Blue,
            success: Color::Green,
            success_fill: Color::Green,
            warning: Color::Yellow,
            warning_soft: Color::Black,
            warning_soft_text: Color::Yellow,
            danger: Color::LightRed,
            danger_fill: Color::Red,
            danger_soft: Color::Black,
            danger_soft_text: Color::LightRed,
            key: Color::DarkGray,
        }
    }

    fn plain(fg: Color) -> Self {
        let r = Color::Reset;
        Palette {
            base: r,
            raised: r,
            header: r,
            stripe: r,
            dimmed: r,
            line: fg,
            text: fg,
            muted: fg,
            label: fg,
            faint: fg,
            primary: fg,
            primary_fill: r,
            primary_soft: r,
            primary_soft_text: fg,
            selection: r,
            success: fg,
            success_fill: r,
            warning: fg,
            warning_soft: r,
            warning_soft_text: fg,
            danger: fg,
            danger_fill: r,
            danger_soft: r,
            danger_soft_text: fg,
            key: r,
        }
    }
}

/// Nearest xterm-256 index: the 6x6x6 cube or the 24-step grey ramp.
pub fn to_256((r, g, b): Rgb) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| {
        (0..6)
            .min_by_key(|&i| (STEPS[i] as i32 - v as i32).abs())
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let dist = |(x, y, z): Rgb| {
        let d = |p: u8, q: u8| (p as i32 - q as i32).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi_ramp = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let grey = 8 + 10 * gi_ramp;
    if dist((grey, grey, grey)) < dist(cube) {
        232 + gi_ramp
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    p: Palette,
    /// No backgrounds: selection and emphasis fall back to modifiers.
    pub plain: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::slate(Depth::TrueColor)
    }
}

impl Theme {
    pub fn slate(depth: Depth) -> Self {
        match depth {
            Depth::TrueColor => Theme {
                p: Palette::from_rgb(|(r, g, b)| Color::Rgb(r, g, b)),
                plain: false,
            },
            Depth::Ansi256 => Theme {
                p: Palette::from_rgb(|rgb| Color::Indexed(to_256(rgb))),
                plain: false,
            },
            Depth::Ansi16 => Theme {
                p: Palette::ansi16(),
                plain: false,
            },
            Depth::Mono => Theme {
                p: Palette::plain(Color::Reset),
                plain: true,
            },
        }
    }

    pub fn high_contrast() -> Self {
        Theme {
            p: Palette::plain(Color::White),
            plain: true,
        }
    }

    pub fn resolve(depth: Depth, high_contrast: bool) -> Self {
        if high_contrast {
            Theme::high_contrast()
        } else {
            Theme::slate(depth)
        }
    }

    pub fn surface(&self, surface: Surface) -> Style {
        let bg = match surface {
            Surface::Base => self.p.base,
            Surface::Raised => self.p.raised,
            Surface::Header => self.p.header,
            Surface::Stripe => self.p.stripe,
            Surface::Dimmed => self.p.dimmed,
        };
        Style::default().bg(bg)
    }

    pub fn text(&self) -> Style {
        Style::default().fg(self.p.text)
    }

    pub fn strong(&self) -> Style {
        self.text().add_modifier(Modifier::BOLD)
    }

    pub fn muted(&self) -> Style {
        Style::default().fg(self.p.muted)
    }

    pub fn label(&self) -> Style {
        let style = Style::default().fg(self.p.label);
        if self.plain {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub fn line(&self) -> Style {
        Style::default().fg(self.p.line)
    }

    /// Text behind an open window.
    pub fn faint(&self) -> Style {
        let style = Style::default().fg(self.p.faint);
        if self.plain {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub fn intent(&self, intent: Intent) -> Style {
        let fg = match intent {
            Intent::Primary => self.p.primary,
            Intent::Success => self.p.success,
            Intent::Warning => self.p.warning,
            Intent::Danger => self.p.danger,
        };
        let style = Style::default().fg(fg);
        if self.plain && matches!(intent, Intent::Primary | Intent::Danger) {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    }

    /// Full-width highlight for the selected row.
    pub fn selected(&self) -> Style {
        if self.plain {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().bg(self.p.selection)
        }
    }

    pub fn tag(&self, intent: Intent, minimal: bool) -> Style {
        if self.plain {
            let style = Style::default().fg(self.p.text);
            return if minimal {
                style
            } else {
                style.add_modifier(Modifier::REVERSED)
            };
        }
        let (fill, soft, soft_text) = match intent {
            Intent::Primary => (
                self.p.primary_fill,
                self.p.primary_soft,
                self.p.primary_soft_text,
            ),
            Intent::Success => (self.p.success_fill, self.p.stripe, self.p.success),
            Intent::Warning => (
                self.p.warning,
                self.p.warning_soft,
                self.p.warning_soft_text,
            ),
            Intent::Danger => (
                self.p.danger_fill,
                self.p.danger_soft,
                self.p.danger_soft_text,
            ),
        };
        if minimal {
            Style::default().bg(soft).fg(soft_text)
        } else {
            Style::default()
                .bg(fill)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        }
    }

    pub fn key(&self) -> Style {
        if self.plain {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().bg(self.p.key).fg(self.p.text)
        }
    }

    pub fn button(&self, primary: bool) -> Style {
        if self.plain {
            let style = Style::default().add_modifier(Modifier::REVERSED);
            return if primary {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };
        }
        if primary {
            Style::default()
                .bg(self.p.primary_fill)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(self.p.key).fg(self.p.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn depth_prefers_no_color_then_colorterm_then_term() {
        assert_eq!(
            Depth::detect(env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")])),
            Depth::Mono
        );
        assert_eq!(
            Depth::detect(env(&[("COLORTERM", "truecolor")])),
            Depth::TrueColor
        );
        assert_eq!(
            Depth::detect(env(&[("TERM", "xterm-256color")])),
            Depth::Ansi256
        );
        assert_eq!(Depth::detect(env(&[("TERM", "xterm")])), Depth::Ansi16);
        assert_eq!(Depth::detect(env(&[("TERM", "dumb")])), Depth::Mono);
        assert_eq!(
            Depth::detect(env(&[("NO_COLOR", "")])),
            Depth::Ansi16,
            "empty NO_COLOR is ignored"
        );
    }

    #[test]
    fn to_256_picks_grey_ramp_for_slate_and_cube_for_blue() {
        assert!((232..=255).contains(&to_256((0x1c, 0x21, 0x27))));
        assert_eq!(to_256((0, 0, 255)), 21);
        assert_eq!(to_256((255, 255, 255)), 231);
    }

    #[test]
    fn plain_themes_use_modifiers_instead_of_backgrounds() {
        for theme in [Theme::high_contrast(), Theme::slate(Depth::Mono)] {
            assert!(theme.plain);
            assert!(theme.selected().add_modifier.contains(Modifier::REVERSED));
            assert_eq!(theme.surface(Surface::Raised).bg, Some(Color::Reset));
        }
        assert_eq!(Theme::high_contrast().text().fg, Some(Color::White));
    }

    #[test]
    fn truecolor_slate_uses_rgb_roles() {
        let theme = Theme::slate(Depth::TrueColor);
        assert_eq!(
            theme.surface(Surface::Base).bg,
            Some(Color::Rgb(0x1c, 0x21, 0x27))
        );
        assert_eq!(theme.selected().bg, Some(Color::Rgb(0x1f, 0x3b, 0x5e)));
    }
}
