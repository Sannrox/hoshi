//! Automation as research progression: adaptive routes, shared queues, then rules.

use crate::buildings::BuildingKind;
use crate::fleets::{Coord, TradeRoute};
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use serde::Deserialize;
use std::fmt;

const DEFAULT: &str = include_str!("../data/automation.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum AutomationError {
    Parse(String),
    DuplicateTier(String),
    UnknownTier(String),
    Locked { tier: String, research: String },
    UnknownPlanet,
    InvalidAmount,
    Conflict(String),
    UnknownRule(u64),
    UnknownBuilding,
    UnknownResource,
}

impl fmt::Display for AutomationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutomationError::Parse(msg) => write!(f, "automation: {msg}"),
            AutomationError::DuplicateTier(id) => write!(f, "automation: duplicate tier {id}"),
            AutomationError::UnknownTier(id) => write!(f, "unknown automation tier {id}"),
            AutomationError::Locked { tier, research } => {
                write!(f, "{tier} needs {research} research")
            }
            AutomationError::UnknownPlanet => write!(f, "planet is not owned"),
            AutomationError::InvalidAmount => write!(f, "amount must be positive and finite"),
            AutomationError::Conflict(msg) => write!(f, "{msg}"),
            AutomationError::UnknownRule(id) => write!(f, "unknown rule #{id}"),
            AutomationError::UnknownBuilding => write!(f, "unknown building"),
            AutomationError::UnknownResource => write!(f, "unknown resource"),
        }
    }
}

impl std::error::Error for AutomationError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AutomationCatalog {
    pub interval_secs: u64,
    #[serde(default)]
    pub tier: Vec<AutomationTier>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AutomationTier {
    pub id: String,
    pub name: String,
    pub research: String,
    pub research_level: u32,
    pub energy: f64,
}

impl AutomationCatalog {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT).expect("bundled automation.toml")
    }

    pub fn parse(raw: &str) -> Result<Self, AutomationError> {
        let catalog: Self =
            toml::from_str(raw).map_err(|e| AutomationError::Parse(e.to_string()))?;
        if catalog.interval_secs == 0 {
            return Err(AutomationError::Parse("interval_secs must be > 0".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for tier in &catalog.tier {
            if !seen.insert(tier.id.as_str()) {
                return Err(AutomationError::DuplicateTier(tier.id.clone()));
            }
            if !tier.energy.is_finite() || tier.energy < 0.0 {
                return Err(AutomationError::Parse(format!(
                    "tier {} energy must be finite and >= 0",
                    tier.id
                )));
            }
        }
        Ok(catalog)
    }

    pub fn get(&self, id: &str) -> Option<&AutomationTier> {
        self.tier.iter().find(|t| t.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleConditionKind {
    StockBelow,
    StockAbove,
}

impl RuleConditionKind {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::StockBelow => "stock_below",
            Self::StockAbove => "stock_above",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, AutomationError> {
        match raw {
            "stock_below" => Ok(Self::StockBelow),
            "stock_above" => Ok(Self::StockAbove),
            _ => Err(AutomationError::Parse(format!("unknown condition {raw}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleCondition {
    pub kind: RuleConditionKind,
    pub planet: Coord,
    pub resource: ResourceKind,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleActionKind {
    Transfer,
    QueueBuild,
}

impl RuleActionKind {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::QueueBuild => "queue_build",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, AutomationError> {
        match raw {
            "transfer" => Ok(Self::Transfer),
            "queue_build" => Ok(Self::QueueBuild),
            _ => Err(AutomationError::Parse(format!("unknown action {raw}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleAction {
    Transfer {
        from: Coord,
        to: Coord,
        resource: ResourceKind,
        amount: f64,
    },
    QueueBuild {
        planet: Coord,
        building: BuildingKind,
    },
}

impl RuleAction {
    pub fn kind(&self) -> RuleActionKind {
        match self {
            Self::Transfer { .. } => RuleActionKind::Transfer,
            Self::QueueBuild { .. } => RuleActionKind::QueueBuild,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Transfer {
                from,
                to,
                resource,
                amount,
            } => format!("transfer {:.0} {} {from}->{to}", amount, resource.as_key()),
            Self::QueueBuild { planet, building } => {
                format!("queue {} at {planet}", building.as_key())
            }
        }
    }
}

impl RuleCondition {
    pub fn describe(&self) -> String {
        format!(
            "{} {} {:.0} at {}",
            self.kind.as_key(),
            self.resource.as_key(),
            self.amount,
            self.planet
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationRule {
    pub id: u64,
    pub enabled: bool,
    pub condition: RuleCondition,
    pub action: RuleAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RulePreview {
    pub rule_id: u64,
    pub would_fire: bool,
    pub detail: String,
}

impl GameState {
    pub fn automation_catalog(&self) -> AutomationCatalog {
        AutomationCatalog::bundled()
    }

    pub fn automation_tier_unlocked(&self, id: &str) -> bool {
        let catalog = AutomationCatalog::bundled();
        let Some(tier) = catalog.get(id) else {
            return false;
        };
        self.tech_level(&tier.research) >= tier.research_level
    }

    pub fn automation_control_energy(&self) -> f64 {
        let catalog = AutomationCatalog::bundled();
        let mut energy = 0.0;
        for tier in &catalog.tier {
            if self.tech_level(&tier.research) < tier.research_level {
                continue;
            }
            energy += match tier.id.as_str() {
                "auto_routes" => tier.energy * self.routes.len() as f64,
                "logistics_rules" => {
                    tier.energy * self.automation_rules.iter().filter(|r| r.enabled).count() as f64
                }
                _ => tier.energy,
            };
        }
        energy
    }

    pub fn adapt_route_cargo(&self, route: &TradeRoute) -> Resources {
        if !self.automation_tier_unlocked("auto_routes") {
            return route.cargo;
        }
        let Some(origin) = self.resources_at(route.origin) else {
            return Resources::ZERO;
        };
        let Some(dest) = self.resources_at(route.dest) else {
            return Resources::ZERO;
        };
        let cap = self.cargo_capacity(route.ships);
        let mut out = Resources::ZERO;
        let mut mass = 0.0;
        for kind in ResourceKind::ALL {
            let want = route.cargo.get(kind);
            if want <= 0.0 {
                continue;
            }
            let surplus = (origin.get(kind) - dest.get(kind)).max(0.0);
            let room = self.storage_room_at(route.dest, kind);
            let take = want.min(surplus).min(origin.get(kind)).min(room);
            if take <= 1e-9 {
                continue;
            }
            if mass + take > cap + 1e-9 {
                let leftover = (cap - mass).max(0.0);
                out.set(kind, leftover);
                break;
            }
            out.set(kind, take);
            mass += take;
        }
        out
    }

    pub fn add_automation_rule(
        &mut self,
        condition: RuleCondition,
        action: RuleAction,
    ) -> Result<u64, AutomationError> {
        self.require_tier("logistics_rules")?;
        validate_amount(&condition, &action)?;
        self.check_owned(condition.planet)?;
        match &action {
            RuleAction::Transfer { from, to, .. } => {
                self.check_owned(*from)?;
                self.check_owned(*to)?;
                if from == to {
                    return Err(AutomationError::Conflict(
                        "transfer origin and destination must differ".into(),
                    ));
                }
            }
            RuleAction::QueueBuild { planet, .. } => {
                self.check_owned(*planet)?;
            }
        }
        if let Some(msg) = self.rule_conflict(&condition, &action) {
            return Err(AutomationError::Conflict(msg));
        }
        self.next_automation_id += 1;
        let id = self.next_automation_id;
        self.automation_rules.push(AutomationRule {
            id,
            enabled: true,
            condition,
            action,
        });
        if self.next_automation_at.is_none() {
            self.next_automation_at = Some(self.last_tick_unix.saturating_add(1));
        }
        Ok(id)
    }

    pub fn toggle_automation_rule(&mut self, id: u64) -> Result<bool, AutomationError> {
        let rule = self
            .automation_rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(AutomationError::UnknownRule(id))?;
        rule.enabled = !rule.enabled;
        let on = rule.enabled;
        if on && self.next_automation_at.is_none() {
            self.next_automation_at = Some(self.last_tick_unix.saturating_add(1));
        }
        if !self.automation_rules.iter().any(|r| r.enabled) {
            self.next_automation_at = None;
        }
        Ok(on)
    }

    pub fn remove_automation_rule(&mut self, id: u64) -> Result<(), AutomationError> {
        let before = self.automation_rules.len();
        self.automation_rules.retain(|r| r.id != id);
        if self.automation_rules.len() == before {
            return Err(AutomationError::UnknownRule(id));
        }
        if !self.automation_rules.iter().any(|r| r.enabled) {
            self.next_automation_at = None;
        }
        Ok(())
    }

    pub fn preview_automation(&self) -> Vec<RulePreview> {
        self.automation_rules
            .iter()
            .map(|rule| self.preview_rule(rule))
            .collect()
    }

    pub fn apply_automation_at(&mut self, at: u64) {
        if let Some(next) = self.next_automation_at
            && at < next
        {
            return;
        }
        if self.automation_tier_unlocked("logistics_rules") {
            let ids: Vec<u64> = self
                .automation_rules
                .iter()
                .filter(|r| r.enabled)
                .map(|r| r.id)
                .collect();
            for id in ids {
                let Some(rule) = self.automation_rules.iter().find(|r| r.id == id).cloned() else {
                    continue;
                };
                if !self.condition_holds(&rule.condition) {
                    continue;
                }
                match self.fire_rule(&rule, at) {
                    Ok(_) => {}
                    Err(err) => self.note_stall(at, format!("rule {id}: skipped ({err})")),
                }
            }
        }
        if self.automation_rules.iter().any(|r| r.enabled) {
            let interval = AutomationCatalog::bundled().interval_secs.max(1);
            self.next_automation_at = Some(at.saturating_add(interval));
        } else {
            self.next_automation_at = None;
        }
    }

    fn require_tier(&self, id: &str) -> Result<(), AutomationError> {
        let catalog = AutomationCatalog::bundled();
        let tier = catalog
            .get(id)
            .ok_or_else(|| AutomationError::UnknownTier(id.into()))?;
        if self.tech_level(&tier.research) < tier.research_level {
            return Err(AutomationError::Locked {
                tier: tier.name.clone(),
                research: tier.research.clone(),
            });
        }
        Ok(())
    }

    fn check_owned(&self, coord: Coord) -> Result<(), AutomationError> {
        if self.resources_at(coord).is_some() {
            Ok(())
        } else {
            Err(AutomationError::UnknownPlanet)
        }
    }

    fn condition_holds(&self, cond: &RuleCondition) -> bool {
        let Some(stock) = self.resources_at(cond.planet) else {
            return false;
        };
        let have = stock.get(cond.resource);
        match cond.kind {
            RuleConditionKind::StockBelow => have + 1e-9 < cond.amount,
            RuleConditionKind::StockAbove => have > cond.amount + 1e-9,
        }
    }

    fn preview_rule(&self, rule: &AutomationRule) -> RulePreview {
        if !rule.enabled {
            return RulePreview {
                rule_id: rule.id,
                would_fire: false,
                detail: "off".into(),
            };
        }
        if !self.automation_tier_unlocked("logistics_rules") {
            return RulePreview {
                rule_id: rule.id,
                would_fire: false,
                detail: "needs logistics_rules research".into(),
            };
        }
        if !self.condition_holds(&rule.condition) {
            return RulePreview {
                rule_id: rule.id,
                would_fire: false,
                detail: "condition not met".into(),
            };
        }
        let detail = match &rule.action {
            RuleAction::Transfer {
                from,
                to,
                resource,
                amount,
            } => format!(
                "transfer {:.0} {} from {from} to {to}",
                amount,
                resource.as_key()
            ),
            RuleAction::QueueBuild { planet, building } => {
                format!("queue {} on {planet}", building.as_key())
            }
        };
        RulePreview {
            rule_id: rule.id,
            would_fire: true,
            detail,
        }
    }

    fn fire_rule(&mut self, rule: &AutomationRule, at: u64) -> Result<(), String> {
        match &rule.action {
            RuleAction::Transfer {
                from,
                to,
                resource,
                amount,
            } => {
                let mut cargo = Resources::ZERO;
                cargo.set(*resource, *amount);
                let ships = self.automation_cargo_ships(*from, cargo);
                self.launch_transport(*from, *to, ships, cargo, 100, at)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            RuleAction::QueueBuild { planet, building } => self
                .enqueue_build_on(*planet, *building, at)
                .map_err(|e| e.to_string()),
        }
    }

    fn automation_cargo_ships(&self, origin: Coord, cargo: Resources) -> Fleet {
        let need = Self::cargo_mass(cargo);
        let mut ships = Fleet::default();
        let parked = if origin == self.home {
            self.fleet
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == origin)
                .map(|c| c.fleet)
                .unwrap_or_default()
        };
        for kind in [ShipKind::LargeCargo, ShipKind::SmallCargo] {
            let have = parked.get(kind);
            if have == 0 {
                continue;
            }
            let cap = self.config.ships.get(kind).cargo;
            if cap <= 0.0 {
                continue;
            }
            let n = ((need / cap).ceil() as u32).max(1).min(have);
            ships.add(kind, n);
            break;
        }
        if ships.total() == 0 {
            ships.add(ShipKind::SmallCargo, 1);
        }
        ships
    }

    fn rule_conflict(&self, condition: &RuleCondition, action: &RuleAction) -> Option<String> {
        for other in self.automation_rules.iter().filter(|r| r.enabled) {
            match (&other.action, action) {
                (
                    RuleAction::Transfer {
                        from: a,
                        to: b,
                        resource: ra,
                        ..
                    },
                    RuleAction::Transfer {
                        from: c,
                        to: d,
                        resource: rb,
                        ..
                    },
                ) if ra == rb && a == d && b == c => {
                    return Some(format!(
                        "conflicts with rule #{} (opposite transfer)",
                        other.id
                    ));
                }
                (
                    RuleAction::QueueBuild {
                        planet: p,
                        building: ba,
                    },
                    RuleAction::QueueBuild {
                        planet: q,
                        building: bb,
                    },
                ) if p == q && ba == bb => {
                    return Some(format!(
                        "conflicts with rule #{} (same queue target)",
                        other.id
                    ));
                }
                _ => {}
            }
            if other.condition.planet == condition.planet
                && other.condition.resource == condition.resource
                && other.condition.kind != condition.kind
            {
                return Some(format!(
                    "conflicts with rule #{} (opposite stock trigger)",
                    other.id
                ));
            }
        }
        None
    }
}

fn validate_amount(condition: &RuleCondition, action: &RuleAction) -> Result<(), AutomationError> {
    if !condition.amount.is_finite() || condition.amount <= 0.0 {
        return Err(AutomationError::InvalidAmount);
    }
    if let RuleAction::Transfer { amount, .. } = action
        && (!amount.is_finite() || *amount <= 0.0)
    {
        return Err(AutomationError::InvalidAmount);
    }
    Ok(())
}

impl GameState {
    fn storage_room_at(&self, coord: Coord, kind: ResourceKind) -> f64 {
        let have = self.resources_at(coord).map(|r| r.get(kind)).unwrap_or(0.0);
        (self.storage_cap_at(coord, kind) - have).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::clock::{Clock, FakeClock};

    fn state_at(now: u64) -> GameState {
        GameState::with_seed(now, BalanceConfig::default(), 1)
    }

    #[test]
    fn bundled_catalog_has_three_ordered_tiers() {
        let catalog = AutomationCatalog::bundled();
        let ids: Vec<_> = catalog.tier.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["auto_routes", "empire_queue", "logistics_rules"]);
        assert!(catalog.interval_secs > 0);
    }

    #[test]
    fn adaptive_cargo_is_fixed_until_auto_routes_research() {
        let mut state = state_at(1_000);
        state.resources.metal = 5_000.0;
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        // no dest colony: adapt returns zero-or-fixed without crash
        let route = TradeRoute {
            id: 1,
            origin: state.home,
            dest,
            ships: {
                let mut f = Fleet::default();
                f.add(ShipKind::SmallCargo, 1);
                f
            },
            cargo: Resources {
                metal: 500.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            speed_percent: 100,
            interval_secs: 60,
            next_at: 1_000,
        };
        let before = state.adapt_route_cargo(&route);
        assert_eq!(before.metal, 500.0);
        state.tech_levels.insert("auto_routes".into(), 1);
        let after = state.adapt_route_cargo(&route);
        assert_eq!(after, Resources::ZERO);
    }

    #[test]
    fn adaptive_cargo_follows_surplus_on_owned_dest() {
        let mut state = state_at(1_000);
        state.resources.metal = 5_000.0;
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 3,
            resources: Resources {
                metal: 100.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        let route = TradeRoute {
            id: 1,
            origin: state.home,
            dest,
            ships: {
                let mut f = Fleet::default();
                f.add(ShipKind::SmallCargo, 1);
                f
            },
            cargo: Resources {
                metal: 500.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            speed_percent: 100,
            interval_secs: 60,
            next_at: 1_000,
        };
        state.tech_levels.insert("auto_routes".into(), 1);
        let cargo = state.adapt_route_cargo(&route);
        assert!(cargo.metal > 0.0, "{cargo:?}");
        assert!(cargo.metal <= 500.0);
    }

    #[test]
    fn rules_need_logistics_rules_research() {
        let mut state = state_at(1_000);
        let err = state
            .add_automation_rule(
                RuleCondition {
                    kind: RuleConditionKind::StockBelow,
                    planet: state.home,
                    resource: ResourceKind::Metal,
                    amount: 100.0,
                },
                RuleAction::QueueBuild {
                    planet: state.home,
                    building: BuildingKind::MetalMine,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("research"), "{err}");
    }

    #[test]
    fn opposite_transfers_are_conflicts() {
        let mut state = state_at(1_000);
        state.tech_levels.insert("logistics_rules".into(), 1);
        let neighbor = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.colonies.push(crate::Colony {
            coord: neighbor,
            kind: crate::PlanetType::Rocky,
            size: 3,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state
            .add_automation_rule(
                RuleCondition {
                    kind: RuleConditionKind::StockAbove,
                    planet: state.home,
                    resource: ResourceKind::Metal,
                    amount: 100.0,
                },
                RuleAction::Transfer {
                    from: state.home,
                    to: neighbor,
                    resource: ResourceKind::Metal,
                    amount: 50.0,
                },
            )
            .unwrap();
        let err = state
            .add_automation_rule(
                RuleCondition {
                    kind: RuleConditionKind::StockBelow,
                    planet: neighbor,
                    resource: ResourceKind::Metal,
                    amount: 10.0,
                },
                RuleAction::Transfer {
                    from: neighbor,
                    to: state.home,
                    resource: ResourceKind::Metal,
                    amount: 50.0,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("opposite"), "{err}");
    }

    #[test]
    fn dry_run_does_not_mutate_and_catch_up_matches_steps() {
        let mut state = state_at(1_000);
        state.tech_levels.insert("logistics_rules".into(), 1);
        state.tech_levels.insert("energy".into(), 1);
        state.resources.metal = 50.0;
        let id = state
            .add_automation_rule(
                RuleCondition {
                    kind: RuleConditionKind::StockBelow,
                    planet: state.home,
                    resource: ResourceKind::Metal,
                    amount: 10_000.0,
                },
                RuleAction::QueueBuild {
                    planet: state.home,
                    building: BuildingKind::MetalMine,
                },
            )
            .unwrap();
        let preview = state.preview_automation();
        assert_eq!(preview[0].rule_id, id);
        assert!(preview[0].would_fire, "{preview:?}");
        assert!(state.build_queue.is_empty());

        let clock = FakeClock::new(1_000);
        let mut stepped = state.clone();
        for t in 1..=5 {
            clock.set(1_000 + t * 300);
            stepped.catch_up(clock.now_unix());
        }
        let mut once = state.clone();
        clock.set(1_000 + 1_500);
        once.catch_up(clock.now_unix());
        assert_eq!(once.build_queue, stepped.build_queue);
        assert!((once.resources.metal - stepped.resources.metal).abs() < 1e-6);
    }

    #[test]
    fn control_energy_is_zero_until_a_tier_unlocks() {
        let mut state = state_at(1_000);
        assert_eq!(state.automation_control_energy(), 0.0);
        state.tech_levels.insert("auto_routes".into(), 1);
        assert!(state.automation_control_energy() >= 0.0);
        state.routes.push(TradeRoute {
            id: 1,
            origin: state.home,
            dest: Coord {
                galaxy: 1,
                system: 1,
                position: 2,
            },
            ships: Fleet::default(),
            cargo: Resources::ZERO,
            speed_percent: 100,
            interval_secs: 60,
            next_at: 1_000,
        });
        assert!(state.automation_control_energy() > 0.0);
    }
}
