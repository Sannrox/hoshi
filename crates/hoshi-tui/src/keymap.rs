//! Named actions with remappable keys. Conflicts are rejected per context.

use ratatui::crossterm::event::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Quit,
    Help,
    NextScreen,
    PrevPlanet,
    NextPlanet,
    SelectNext,
    SelectPrev,
    Confirm,
    Cancel,
    CopySeed,
    ShowAway,
    Increase,
    Decrease,
    SendProbe,
    DeleteSlot,
    RenameSlot,
    Toggle,
    CursorLeft,
    CursorRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpContext {
    Overview,
    Planet,
    Shipyard,
    Wonder,
    Prestige,
    Achievements,
    Market,
    Diplomacy,
    Laws,
    Reports,
    Map,
    Dispatch,
    Away,
    Story,
    ReportView,
    TitleMenu,
    TitleNew,
    TitleLoad,
    TitleDelete,
    TitleRename,
    TitleSettings,
    TitleCredits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    keys: Vec<(Binding, Vec<KeyCode>)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Binding {
    const REMAPPABLE: &[Binding] = &[
        Binding::Quit,
        Binding::Help,
        Binding::NextScreen,
        Binding::PrevPlanet,
        Binding::NextPlanet,
        Binding::SelectNext,
        Binding::SelectPrev,
        Binding::Confirm,
        Binding::Cancel,
        Binding::CopySeed,
        Binding::ShowAway,
        Binding::Increase,
        Binding::Decrease,
        Binding::SendProbe,
        Binding::DeleteSlot,
        Binding::RenameSlot,
        Binding::Toggle,
        Binding::CursorLeft,
        Binding::CursorRight,
    ];

    fn config_key(self) -> &'static str {
        match self {
            Binding::Quit => "quit",
            Binding::Help => "help",
            Binding::NextScreen => "next_screen",
            Binding::PrevPlanet => "prev_planet",
            Binding::NextPlanet => "next_planet",
            Binding::SelectNext => "select_next",
            Binding::SelectPrev => "select_prev",
            Binding::Confirm => "confirm",
            Binding::Cancel => "cancel",
            Binding::CopySeed => "copy_seed",
            Binding::ShowAway => "show_away",
            Binding::Increase => "increase",
            Binding::Decrease => "decrease",
            Binding::SendProbe => "send_probe",
            Binding::DeleteSlot => "delete_slot",
            Binding::RenameSlot => "rename_slot",
            Binding::Toggle => "toggle",
            Binding::CursorLeft => "cursor_left",
            Binding::CursorRight => "cursor_right",
        }
    }

    fn from_config_key(name: &str) -> Option<Self> {
        Binding::REMAPPABLE
            .iter()
            .copied()
            .find(|b| b.config_key() == name)
    }

    fn default_keys(self) -> Vec<KeyCode> {
        match self {
            Binding::Quit => vec![KeyCode::Char('q')],
            Binding::Help => vec![KeyCode::Char('?')],
            Binding::NextScreen => vec![KeyCode::Tab],
            Binding::PrevPlanet => vec![KeyCode::Char('[')],
            Binding::NextPlanet => vec![KeyCode::Char(']')],
            Binding::SelectNext => vec![KeyCode::Char('j'), KeyCode::Down],
            Binding::SelectPrev => vec![KeyCode::Char('k'), KeyCode::Up],
            Binding::Confirm => vec![KeyCode::Enter],
            Binding::Cancel => vec![KeyCode::Char('x'), KeyCode::Esc],
            Binding::CopySeed => vec![KeyCode::Char('y')],
            Binding::ShowAway => vec![KeyCode::Char('w')],
            Binding::Increase => vec![KeyCode::Char('+')],
            Binding::Decrease => vec![KeyCode::Char('-')],
            Binding::SendProbe => vec![KeyCode::Char('s')],
            Binding::DeleteSlot => vec![KeyCode::Char('d')],
            Binding::RenameSlot => vec![KeyCode::Char('r')],
            Binding::Toggle => vec![KeyCode::Char(' ')],
            Binding::CursorLeft => vec![KeyCode::Char('h'), KeyCode::Left],
            Binding::CursorRight => vec![KeyCode::Char('l'), KeyCode::Right],
        }
    }
}

impl HelpContext {
    fn bindings(self) -> &'static [Binding] {
        match self {
            HelpContext::Overview => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::PrevPlanet,
                Binding::NextPlanet,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::Cancel,
                Binding::CopySeed,
                Binding::ShowAway,
                Binding::CursorLeft,
                Binding::CursorRight,
            ],
            HelpContext::Planet | HelpContext::Shipyard | HelpContext::Wonder => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::PrevPlanet,
                Binding::NextPlanet,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::Cancel,
                Binding::CopySeed,
            ],
            HelpContext::Prestige => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::Cancel,
                Binding::CopySeed,
            ],
            HelpContext::Achievements => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::CopySeed,
            ],
            HelpContext::Market => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::Increase,
                Binding::Decrease,
                Binding::CopySeed,
            ],
            HelpContext::Diplomacy | HelpContext::Laws => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::CopySeed,
            ],
            HelpContext::Reports => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
                Binding::SendProbe,
                Binding::CopySeed,
            ],
            HelpContext::Map => &[
                Binding::Help,
                Binding::Quit,
                Binding::NextScreen,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::CursorLeft,
                Binding::CursorRight,
                Binding::Confirm,
                Binding::Cancel,
                Binding::CopySeed,
            ],
            HelpContext::Dispatch => &[
                Binding::Help,
                Binding::Quit,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Increase,
                Binding::Decrease,
                Binding::Confirm,
                Binding::Cancel,
            ],
            HelpContext::Away | HelpContext::ReportView => &[
                Binding::Help,
                Binding::Quit,
                Binding::Confirm,
                Binding::Cancel,
            ],
            HelpContext::Story => &[Binding::Help, Binding::Quit],
            HelpContext::TitleMenu => &[
                Binding::Help,
                Binding::Quit,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::Confirm,
            ],
            HelpContext::TitleNew => &[
                Binding::Help,
                Binding::Cancel,
                Binding::Confirm,
                Binding::NextScreen,
                Binding::Toggle,
            ],
            HelpContext::TitleLoad => &[
                Binding::Help,
                Binding::Cancel,
                Binding::Confirm,
                Binding::SelectNext,
                Binding::SelectPrev,
                Binding::DeleteSlot,
                Binding::RenameSlot,
            ],
            HelpContext::TitleDelete => &[
                Binding::Help,
                Binding::Cancel,
                Binding::Confirm,
                Binding::DeleteSlot,
            ],
            HelpContext::TitleRename => &[Binding::Help, Binding::Cancel, Binding::Confirm],
            HelpContext::TitleSettings => &[
                Binding::Help,
                Binding::Cancel,
                Binding::Confirm,
                Binding::Toggle,
                Binding::SelectNext,
                Binding::SelectPrev,
            ],
            HelpContext::TitleCredits => &[Binding::Help, Binding::Confirm, Binding::Cancel],
        }
    }

    fn label(binding: Binding) -> &'static str {
        match binding {
            Binding::Quit => "quit",
            Binding::Help => "help",
            Binding::NextScreen => "next screen / next field",
            Binding::PrevPlanet => "previous planet",
            Binding::NextPlanet => "next planet",
            Binding::SelectNext => "select next / move down",
            Binding::SelectPrev => "select previous / move up",
            Binding::Confirm => "confirm / activate",
            Binding::Cancel => "cancel / back / dismiss",
            Binding::CopySeed => "copy galaxy seed",
            Binding::ShowAway => "reopen away summary",
            Binding::Increase => "increase amount",
            Binding::Decrease => "decrease amount",
            Binding::SendProbe => "send spy probe",
            Binding::DeleteSlot => "delete save",
            Binding::RenameSlot => "rename save",
            Binding::Toggle => "toggle / cycle",
            Binding::CursorLeft => "cursor left / previous branch",
            Binding::CursorRight => "cursor right / next branch",
        }
    }
}

impl Keymap {
    pub fn defaults() -> Self {
        Self {
            keys: Binding::REMAPPABLE
                .iter()
                .copied()
                .map(|b| (b, b.default_keys()))
                .collect(),
        }
    }

    pub fn from_overrides<'a, I>(pairs: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut km = Self::defaults();
        for (name, value) in pairs {
            let Some(binding) = Binding::from_config_key(name) else {
                return Err(format!("unknown key action '{name}'"));
            };
            let codes = parse_key_list(value)?;
            if codes.is_empty() {
                return Err(format!("empty binding for {name}"));
            }
            km.set(binding, codes);
        }
        km.validate()?;
        Ok(km)
    }

    fn set(&mut self, binding: Binding, codes: Vec<KeyCode>) {
        if let Some(slot) = self.keys.iter_mut().find(|(b, _)| *b == binding) {
            slot.1 = codes;
        }
    }

    fn keys_of(&self, binding: Binding) -> &[KeyCode] {
        self.keys
            .iter()
            .find(|(b, _)| *b == binding)
            .map(|(_, k)| k.as_slice())
            .unwrap_or(&[])
    }

    pub fn matches(&self, binding: Binding, code: KeyCode) -> bool {
        self.keys_of(binding).contains(&code)
    }

    pub fn lookup(&self, ctx: HelpContext, code: KeyCode) -> Option<Binding> {
        ctx.bindings()
            .iter()
            .copied()
            .find(|b| self.matches(*b, code))
    }

    pub fn display(&self, binding: Binding) -> String {
        self.keys_of(binding)
            .iter()
            .map(format_key)
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn primary(&self, binding: Binding) -> String {
        self.keys_of(binding)
            .first()
            .map(format_key)
            .unwrap_or_default()
    }

    pub fn help_lines(&self, ctx: HelpContext) -> Vec<String> {
        let mut lines = vec!["keys for this screen".into(), String::new()];
        let mut rows: Vec<(String, String)> = ctx
            .bindings()
            .iter()
            .copied()
            .map(|b| (self.display(b), HelpContext::label(b).to_string()))
            .collect();
        if ctx == HelpContext::Story {
            rows.push(("1-9".into(), "choose story option".into()));
        }
        if ctx == HelpContext::TitleNew {
            rows.push(("down/up".into(), "next or previous field".into()));
            rows.push(("type".into(), "edit name or seed".into()));
            rows.push(("backspace".into(), "delete character".into()));
        }
        if ctx == HelpContext::TitleRename {
            rows.push(("type".into(), "edit slot name".into()));
            rows.push(("backspace".into(), "delete character".into()));
        }
        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (keys, label) in rows {
            lines.push(format!("  {keys:<width$}  {label}"));
        }
        lines.push(String::new());
        lines.push(format!(
            "  {} closes help",
            [Binding::Help, Binding::Cancel, Binding::Confirm]
                .into_iter()
                .map(|b| self.display(b))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
        lines
    }

    pub fn to_config_lines(&self) -> String {
        let mut out = String::from("\n");
        for binding in Binding::REMAPPABLE {
            let value = self
                .keys_of(*binding)
                .iter()
                .map(format_key)
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!("keys.{} = \"{value}\"\n", binding.config_key()));
        }
        out
    }

    fn validate(&self) -> Result<(), String> {
        for ctx in [
            HelpContext::Overview,
            HelpContext::Planet,
            HelpContext::Shipyard,
            HelpContext::Wonder,
            HelpContext::Prestige,
            HelpContext::Achievements,
            HelpContext::Market,
            HelpContext::Diplomacy,
            HelpContext::Laws,
            HelpContext::Reports,
            HelpContext::Map,
            HelpContext::Dispatch,
            HelpContext::Away,
            HelpContext::Story,
            HelpContext::ReportView,
            HelpContext::TitleMenu,
            HelpContext::TitleNew,
            HelpContext::TitleLoad,
            HelpContext::TitleDelete,
            HelpContext::TitleRename,
            HelpContext::TitleSettings,
            HelpContext::TitleCredits,
        ] {
            let mut seen: Vec<(KeyCode, Binding)> = Vec::new();
            for binding in ctx.bindings() {
                for code in self.keys_of(*binding) {
                    if let Some((_, other)) = seen.iter().find(|(c, _)| c == code)
                        && *other != *binding
                    {
                        return Err(format!(
                            "conflict: {} and {} both use {}",
                            other.config_key(),
                            binding.config_key(),
                            format_key(code)
                        ));
                    }
                    seen.push((*code, *binding));
                }
            }
            if ctx == HelpContext::Story {
                for d in b'1'..=b'9' {
                    let code = KeyCode::Char(d as char);
                    if let Some((_, other)) = seen.iter().find(|(c, _)| *c == code) {
                        return Err(format!(
                            "conflict: {} uses {}, reserved for story choices",
                            other.config_key(),
                            format_key(&code)
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn parse_key_list(value: &str) -> Result<Vec<KeyCode>, String> {
    let mut codes = Vec::new();
    for part in value.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        codes.push(parse_key(token)?);
    }
    Ok(codes)
}

fn parse_key(token: &str) -> Result<KeyCode, String> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "tab" => Ok(KeyCode::Tab),
        "enter" | "return" => Ok(KeyCode::Enter),
        "esc" | "escape" => Ok(KeyCode::Esc),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),
        "space" => Ok(KeyCode::Char(' ')),
        "plus" => Ok(KeyCode::Char('+')),
        "minus" => Ok(KeyCode::Char('-')),
        "?" => Ok(KeyCode::Char('?')),
        token if token.chars().count() == 1 => {
            let c = token.chars().next().unwrap();
            Ok(KeyCode::Char(c))
        }
        _ => Err(format!("unknown key '{token}'")),
    }
}

fn format_key(code: &KeyCode) -> String {
    match code {
        KeyCode::Tab => "tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_conflicts() {
        Keymap::defaults().validate().unwrap();
    }

    #[test]
    fn remap_quit_is_accepted() {
        let km = Keymap::from_overrides([("quit", "p")]).unwrap();
        assert!(km.matches(Binding::Quit, KeyCode::Char('p')));
        assert!(!km.matches(Binding::Quit, KeyCode::Char('q')));
        assert_eq!(
            km.lookup(HelpContext::Overview, KeyCode::Char('p')),
            Some(Binding::Quit)
        );
    }

    #[test]
    fn conflicting_remap_is_rejected() {
        let err = Keymap::from_overrides([("quit", "j")]).unwrap_err();
        assert!(err.contains("conflict"), "{err}");
        assert!(err.contains("select_next") || err.contains("quit"), "{err}");
    }

    #[test]
    fn story_digits_cannot_be_stolen() {
        let err = Keymap::from_overrides([("help", "1")]).unwrap_err();
        assert!(err.contains("story"), "{err}");
    }

    #[test]
    fn title_new_leaves_letters_free_to_type() {
        let km = Keymap::defaults();
        assert_eq!(km.lookup(HelpContext::TitleNew, KeyCode::Char('s')), None);
        assert_eq!(km.lookup(HelpContext::TitleNew, KeyCode::Char('q')), None);
        assert_eq!(km.lookup(HelpContext::TitleNew, KeyCode::Char('j')), None);
        assert_eq!(
            km.lookup(HelpContext::TitleNew, KeyCode::Char('x')),
            Some(Binding::Cancel)
        );
    }

    #[test]
    fn help_lists_current_screen_actions() {
        let km = Keymap::defaults();
        let overview = km.help_lines(HelpContext::Overview).join("\n");
        assert!(overview.contains("quit"), "{overview}");
        assert!(overview.contains("?"), "{overview}");
        assert!(overview.contains("copy galaxy seed"), "{overview}");
        let market = km.help_lines(HelpContext::Market).join("\n");
        assert!(market.contains("increase amount"), "{market}");
        assert!(!market.contains("send spy probe"), "{market}");
        let reports = km.help_lines(HelpContext::Reports).join("\n");
        assert!(reports.contains("send spy probe"), "{reports}");
    }

    #[test]
    fn unknown_action_is_rejected() {
        let err = Keymap::from_overrides([("dash", "z")]).unwrap_err();
        assert!(err.contains("unknown"), "{err}");
    }
}
