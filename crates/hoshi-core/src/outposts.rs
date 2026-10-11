//! NPC pirate outposts. Strength is a function of player combat points.

use crate::combat::next_u64;
use crate::fleets::Coord;
use crate::planets::{PlanetType, planet_profile};
use crate::ships::{Fleet, ShipKind};

/// A pirate-held world. Conquered outposts become player colonies.
#[derive(Debug, Clone, PartialEq)]
pub struct Outpost {
    pub coord: Coord,
    pub kind: PlanetType,
    pub size: u8,
    pub defences: Fleet,
    pub garrison: u32,
    pub rare: Option<String>,
    pub last_rebuild_at: u64,
}

impl Outpost {
    pub fn next_rebuild_at(&self, interval: u64) -> u64 {
        self.last_rebuild_at.saturating_add(interval.max(1))
    }
}

/// One rare-planet bonus row from balance config.
#[derive(Debug, Clone, PartialEq)]
pub struct RarePlanetBonus {
    pub id: String,
    pub name: String,
    pub weight: u32,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub energy: f64,
    /// Speeds the empire shipyard (ships queue at home).
    pub shipyard_speed: f64,
    pub fields: u8,
}

impl RarePlanetBonus {
    /// Short labels for the planet screen (`metal +25%`, `fields +20`).
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.metal > 0.0 {
            parts.push(format!("metal +{:.0}%", self.metal * 100.0));
        }
        if self.crystal > 0.0 {
            parts.push(format!("crystal +{:.0}%", self.crystal * 100.0));
        }
        if self.deuterium > 0.0 {
            parts.push(format!("deuterium +{:.0}%", self.deuterium * 100.0));
        }
        if self.energy > 0.0 {
            parts.push(format!("energy +{:.0}%", self.energy * 100.0));
        }
        if self.shipyard_speed > 0.0 {
            parts.push(format!("shipyard +{:.0}%", self.shipyard_speed * 100.0));
        }
        if self.fields > 0 {
            parts.push(format!("fields +{}", self.fields));
        }
        parts.join("  ")
    }
}

/// Outpost count, scaling, rebuild and rare table.
#[derive(Debug, Clone, PartialEq)]
pub struct OutpostConfig {
    pub count: u32,
    pub min_ships: u32,
    pub strength_factor: f64,
    pub min_garrison: u32,
    pub garrison_factor: f64,
    pub rebuild_secs: u64,
    pub variance: f64,
    pub troop_strength: u32,
    pub rare_chance: f64,
    pub rare: Vec<RarePlanetBonus>,
}

impl Default for OutpostConfig {
    fn default() -> Self {
        Self {
            count: 8,
            min_ships: 1,
            strength_factor: 0.4,
            min_garrison: 2,
            garrison_factor: 0.5,
            rebuild_secs: 86_400,
            variance: 0.2,
            troop_strength: 5,
            rare_chance: 0.25,
            rare: vec![
                RarePlanetBonus {
                    id: "rich_vein".into(),
                    name: "Rich vein".into(),
                    weight: 3,
                    metal: 0.25,
                    crystal: 0.0,
                    deuterium: 0.0,
                    energy: 0.0,
                    shipyard_speed: 0.0,
                    fields: 0,
                },
                RarePlanetBonus {
                    id: "crystal_garden".into(),
                    name: "Crystal garden".into(),
                    weight: 2,
                    metal: 0.0,
                    crystal: 0.30,
                    deuterium: 0.0,
                    energy: 0.0,
                    shipyard_speed: 0.0,
                    fields: 0,
                },
                RarePlanetBonus {
                    id: "shipyard_world".into(),
                    name: "Shipyard world".into(),
                    weight: 1,
                    metal: 0.0,
                    crystal: 0.0,
                    deuterium: 0.0,
                    energy: 0.0,
                    shipyard_speed: 0.25,
                    fields: 0,
                },
                RarePlanetBonus {
                    id: "wide_fields".into(),
                    name: "Wide fields".into(),
                    weight: 2,
                    metal: 0.0,
                    crystal: 0.0,
                    deuterium: 0.0,
                    energy: 0.0,
                    shipyard_speed: 0.0,
                    fields: 20,
                },
            ],
        }
    }
}

/// Defence light-fighter count from the player's fighter-equivalent fleet.
pub fn scaled_defence(fighter_eq: f64, cfg: &OutpostConfig) -> Fleet {
    let scaled = (fighter_eq * cfg.strength_factor).floor().max(0.0) as u32;
    let n = scaled.max(cfg.min_ships);
    let mut fleet = Fleet::default();
    if n > 0 {
        fleet.add(ShipKind::LightFighter, n);
    }
    fleet
}

pub fn scaled_garrison(fighter_eq: f64, cfg: &OutpostConfig) -> u32 {
    let scaled = (fighter_eq * cfg.garrison_factor).floor().max(0.0) as u32;
    scaled.max(cfg.min_garrison)
}

/// Scatter outposts outside the home system so nearby colonize/transport slots stay free.
pub fn generate_outposts(
    galaxy_seed: u64,
    now: u64,
    cfg: &OutpostConfig,
    occupied: &[Coord],
) -> Vec<Outpost> {
    let mut seed = galaxy_seed ^ 0x0A11_0057_0A11_0057;
    let mut out = Vec::new();
    let mut used: Vec<Coord> = occupied.to_vec();
    let mut attempts = 0;
    while out.len() < cfg.count as usize && attempts < 2_000 {
        attempts += 1;
        let roll = next_u64(&mut seed);
        let system = 2 + (roll % 80) as u16;
        let position = 3 + ((roll >> 16) % 6) as u16;
        let coord = Coord {
            galaxy: 1,
            system,
            position,
        };
        if used.contains(&coord) {
            continue;
        }
        used.push(coord);
        let (kind, size) = planet_profile(galaxy_seed, coord);
        let rare = pick_rare(&mut seed, cfg);
        let extra = rare
            .as_ref()
            .and_then(|id| cfg.rare.iter().find(|row| row.id == *id))
            .map(|row| row.fields)
            .unwrap_or(0);
        out.push(Outpost {
            coord,
            kind,
            size: size.saturating_add(extra),
            defences: scaled_defence(0.0, cfg),
            garrison: scaled_garrison(0.0, cfg),
            rare,
            last_rebuild_at: now,
        });
    }
    out
}

fn pick_rare(seed: &mut u64, cfg: &OutpostConfig) -> Option<String> {
    if cfg.rare.is_empty() || cfg.rare_chance <= 0.0 {
        return None;
    }
    let span = (cfg.rare_chance.clamp(0.0, 1.0) * 10_000.0).round() as u64;
    if next_u64(seed) % 10_000 >= span {
        return None;
    }
    let total: u64 = cfg
        .rare
        .iter()
        .map(|row| u64::from(row.weight.max(1)))
        .sum();
    if total == 0 {
        return None;
    }
    let mut pick = next_u64(seed) % total;
    for row in &cfg.rare {
        let w = u64::from(row.weight.max(1));
        if pick < w {
            return Some(row.id.clone());
        }
        pick -= w;
    }
    cfg.rare.last().map(|row| row.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_repeats_for_a_seed_and_skips_home() {
        let cfg = OutpostConfig::default();
        let home = Coord::HOME;
        let a = generate_outposts(7, 0, &cfg, &[home]);
        let b = generate_outposts(7, 0, &cfg, &[home]);
        assert_eq!(a, b);
        assert_eq!(a.len(), cfg.count as usize);
        assert!(a.iter().all(|o| o.coord != home && o.coord.system >= 2));
        let other = generate_outposts(8, 0, &cfg, &[home]);
        assert_ne!(a[0].coord, other[0].coord);
    }

    #[test]
    fn zero_progress_uses_the_configured_floor() {
        let cfg = OutpostConfig::default();
        let fleet = scaled_defence(0.0, &cfg);
        assert_eq!(fleet.get(ShipKind::LightFighter), cfg.min_ships);
        assert_eq!(scaled_garrison(0.0, &cfg), cfg.min_garrison);
        let strong = scaled_defence(20.0, &cfg);
        assert!(strong.get(ShipKind::LightFighter) > cfg.min_ships);
    }
}
