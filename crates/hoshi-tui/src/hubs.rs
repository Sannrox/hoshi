//! Six hubs group the screens. Digits 1-6 open a hub; Tab walks screens in hub order.

use crate::app::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hub {
    Bridge,
    Colony,
    Fleet,
    Trade,
    Empire,
    Records,
}

impl Hub {
    pub const ALL: [Hub; 6] = [
        Hub::Bridge,
        Hub::Colony,
        Hub::Fleet,
        Hub::Trade,
        Hub::Empire,
        Hub::Records,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Hub::Bridge => "Bridge",
            Hub::Colony => "Colony",
            Hub::Fleet => "Fleet",
            Hub::Trade => "Trade",
            Hub::Empire => "Empire",
            Hub::Records => "Records",
        }
    }

    pub fn screens(self) -> &'static [Screen] {
        match self {
            Hub::Bridge => &[
                Screen::Cockpit,
                Screen::Goals,
                Screen::Daily,
                Screen::Achievements,
            ],
            Hub::Colony => &[
                Screen::Planet,
                Screen::Production,
                Screen::Maintenance,
                Screen::Overview,
                Screen::Wonder,
            ],
            Hub::Fleet => &[Screen::Shipyard, Screen::Map, Screen::Ops, Screen::Reports],
            Hub::Trade => &[Screen::Market, Screen::Finance, Screen::Kpis],
            Hub::Empire => &[
                Screen::Diplomacy,
                Screen::Laws,
                Screen::Staff,
                Screen::Automation,
                Screen::Incidents,
            ],
            Hub::Records => &[Screen::History, Screen::Scenarios, Screen::Prestige],
        }
    }

    pub fn of(screen: Screen) -> Hub {
        Hub::ALL
            .into_iter()
            .find(|hub| hub.screens().contains(&screen))
            .unwrap_or(Hub::Bridge)
    }

    pub fn from_digit(c: char) -> Option<Hub> {
        let i = c.to_digit(10)?.checked_sub(1)? as usize;
        Hub::ALL.get(i).copied()
    }
}

pub fn label(screen: Screen) -> &'static str {
    match screen {
        Screen::Cockpit => "Dashboard",
        Screen::Goals => "Goals",
        Screen::Daily => "Daily",
        Screen::Achievements => "Feats",
        Screen::Planet => "Buildings",
        Screen::Production => "Production",
        Screen::Maintenance => "Maintenance",
        Screen::Overview => "Research",
        Screen::Wonder => "Wonder",
        Screen::Shipyard => "Shipyard",
        Screen::Map => "Star map",
        Screen::Ops => "Ops",
        Screen::Reports => "Intel",
        Screen::Market => "Market",
        Screen::Finance => "Finance",
        Screen::Kpis => "KPIs",
        Screen::Diplomacy => "Diplomacy",
        Screen::Laws => "Laws",
        Screen::Staff => "Staff",
        Screen::Automation => "Automation",
        Screen::Incidents => "Incidents",
        Screen::History => "History",
        Screen::Scenarios => "Scenarios",
        Screen::Prestige => "Legacy",
    }
}

/// Next (or previous) screen in hub order, wrapping across hubs.
pub fn step(screen: Screen, forward: bool) -> Screen {
    let order: Vec<Screen> = Hub::ALL
        .iter()
        .flat_map(|hub| hub.screens().iter().copied())
        .collect();
    let i = order.iter().position(|s| *s == screen).unwrap_or(0);
    let n = order.len();
    order[if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    }]
}

/// Screen a hub key opens: the hub's first screen, or the next tab when already inside it.
pub fn enter(current: Screen, hub: Hub) -> Screen {
    let screens = hub.screens();
    match screens.iter().position(|s| *s == current) {
        Some(i) => screens[(i + 1) % screens.len()],
        None => screens[0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: [Screen; 24] = [
        Screen::Overview,
        Screen::Planet,
        Screen::Shipyard,
        Screen::Wonder,
        Screen::Prestige,
        Screen::Achievements,
        Screen::Daily,
        Screen::Goals,
        Screen::Production,
        Screen::Maintenance,
        Screen::Market,
        Screen::Diplomacy,
        Screen::Laws,
        Screen::Reports,
        Screen::History,
        Screen::Scenarios,
        Screen::Staff,
        Screen::Ops,
        Screen::Automation,
        Screen::Incidents,
        Screen::Finance,
        Screen::Kpis,
        Screen::Map,
        Screen::Cockpit,
    ];

    #[test]
    fn every_screen_sits_in_exactly_one_hub() {
        for screen in EVERY {
            let homes = Hub::ALL
                .iter()
                .filter(|hub| hub.screens().contains(&screen))
                .count();
            assert_eq!(homes, 1, "{screen:?}");
        }
    }

    #[test]
    fn tab_walks_every_screen_once_and_wraps() {
        let mut seen = vec![Screen::Cockpit];
        let mut screen = step(Screen::Cockpit, true);
        while screen != Screen::Cockpit {
            assert!(!seen.contains(&screen), "{screen:?} twice");
            seen.push(screen);
            screen = step(screen, true);
        }
        assert_eq!(seen.len(), EVERY.len());
        assert_eq!(step(Screen::Achievements, true), Screen::Planet);
        assert_eq!(step(Screen::Cockpit, false), Screen::Prestige);
    }

    #[test]
    fn hub_keys_open_then_cycle_inside_the_hub() {
        assert_eq!(Hub::from_digit('2'), Some(Hub::Colony));
        assert_eq!(Hub::from_digit('0'), None);
        assert_eq!(Hub::from_digit('7'), None);
        assert_eq!(enter(Screen::Cockpit, Hub::Colony), Screen::Planet);
        assert_eq!(enter(Screen::Planet, Hub::Colony), Screen::Production);
        assert_eq!(enter(Screen::Wonder, Hub::Colony), Screen::Planet);
        assert_eq!(enter(Screen::Market, Hub::Trade), Screen::Finance);
        assert_eq!(enter(Screen::Finance, Hub::Trade), Screen::Kpis);
        assert_eq!(enter(Screen::Kpis, Hub::Trade), Screen::Market);
        assert_eq!(enter(Screen::Automation, Hub::Empire), Screen::Incidents);
        assert_eq!(enter(Screen::Incidents, Hub::Empire), Screen::Diplomacy);
    }
}
