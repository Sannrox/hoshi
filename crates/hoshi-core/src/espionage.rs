//! Spy-probe reports. Score, detection and destruction come from config.

use crate::combat::next_u64;
use crate::fleets::Coord;
use crate::planets::PlanetType;
use crate::ships::Fleet;
use crate::state::Resources;

/// One named detail band. Highest `min_score` the probe score still meets wins.
#[derive(Debug, Clone, PartialEq)]
pub struct EspionageTier {
    pub id: String,
    pub min_score: f64,
}

/// Probe score, detection chance and destruction chance.
#[derive(Debug, Clone, PartialEq)]
pub struct EspionageConfig {
    /// Outpost counter-espionage used in the score and detection formulae.
    pub target_level: f64,
    /// `score += probes * probe_weight`.
    pub probe_weight: f64,
    /// `score += (own_espionage - target_level) * level_weight`.
    pub level_weight: f64,
    pub detect_base: f64,
    pub detect_per_probe: f64,
    pub detect_per_level: f64,
    /// Per-probe destruction chance after a detection.
    pub destroy_chance: f64,
    pub tiers: Vec<EspionageTier>,
}

impl Default for EspionageConfig {
    fn default() -> Self {
        Self {
            target_level: 1.0,
            probe_weight: 1.0,
            level_weight: 1.0,
            detect_base: 0.05,
            detect_per_probe: 0.08,
            detect_per_level: 0.12,
            destroy_chance: 0.5,
            tiers: vec![
                EspionageTier {
                    id: "activity".into(),
                    min_score: 0.0,
                },
                EspionageTier {
                    id: "resources".into(),
                    min_score: 1.0,
                },
                EspionageTier {
                    id: "fleet".into(),
                    min_score: 2.0,
                },
                EspionageTier {
                    id: "defences".into(),
                    min_score: 3.0,
                },
                EspionageTier {
                    id: "garrison".into(),
                    min_score: 4.0,
                },
            ],
        }
    }
}

/// Stored intel from one probe arrival.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyReport {
    pub id: u64,
    pub target: Coord,
    pub created_at: u64,
    pub probes_sent: u32,
    pub probes_lost: u32,
    pub detected: bool,
    pub score: f64,
    pub tier: String,
    pub resources: Option<Resources>,
    pub fleet: Option<Fleet>,
    pub defences: Option<Fleet>,
    pub garrison: Option<u32>,
    pub kind: Option<PlanetType>,
    pub size: Option<u8>,
}

impl SpyReport {
    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.created_at)
    }
}

/// Outcome of one probe wave. Detection and losses are drawn from `rng`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyOutcome {
    pub score: f64,
    pub tier: String,
    pub detected: bool,
    pub probes_lost: u32,
}

pub fn spy_score(probes: u32, own_level: f64, cfg: &EspionageConfig) -> f64 {
    f64::from(probes) * cfg.probe_weight + (own_level - cfg.target_level) * cfg.level_weight
}

pub fn spy_tier(score: f64, cfg: &EspionageConfig) -> Option<&EspionageTier> {
    cfg.tiers
        .iter()
        .filter(|tier| score + 1e-12 >= tier.min_score)
        .max_by(|a, b| {
            a.min_score
                .partial_cmp(&b.min_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

pub fn detection_chance(probes: u32, own_level: f64, cfg: &EspionageConfig) -> f64 {
    let raw = cfg.detect_base + f64::from(probes) * cfg.detect_per_probe
        - (own_level - cfg.target_level) * cfg.detect_per_level;
    raw.clamp(0.0, 1.0)
}

fn unit_f64(rng: &mut u64) -> f64 {
    (next_u64(rng) as f64) / ((u64::MAX as f64) + 1.0)
}

/// Draw detection and per-probe destruction. Same seed yields the same outcome.
pub fn resolve_spy(
    probes: u32,
    own_level: f64,
    rng: &mut u64,
    cfg: &EspionageConfig,
) -> SpyOutcome {
    let score = spy_score(probes, own_level, cfg);
    let tier = spy_tier(score, cfg)
        .map(|t| t.id.clone())
        .unwrap_or_else(|| "activity".into());
    let chance = detection_chance(probes, own_level, cfg);
    let detected = unit_f64(rng) < chance;
    let mut probes_lost = 0;
    if detected {
        for _ in 0..probes {
            if unit_f64(rng) < cfg.destroy_chance.clamp(0.0, 1.0) {
                probes_lost += 1;
            }
        }
    }
    SpyOutcome {
        score,
        tier,
        detected,
        probes_lost,
    }
}

pub fn reveals(tier_id: &str, field: &str, cfg: &EspionageConfig) -> bool {
    let Some(won) = cfg.tiers.iter().find(|t| t.id == tier_id) else {
        return false;
    };
    cfg.tiers
        .iter()
        .any(|t| t.id == field && won.min_score + 1e-12 >= t.min_score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_adds_probes_and_research_gap() {
        let cfg = EspionageConfig::default();
        assert!((spy_score(1, 0.0, &cfg) - 0.0).abs() < 1e-12);
        assert!((spy_score(1, 2.0, &cfg) - 2.0).abs() < 1e-12);
        assert!((spy_score(5, 0.0, &cfg) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn tiers_follow_min_score() {
        let cfg = EspionageConfig::default();
        assert_eq!(spy_tier(0.0, &cfg).unwrap().id, "activity");
        assert_eq!(spy_tier(1.0, &cfg).unwrap().id, "resources");
        assert_eq!(spy_tier(2.0, &cfg).unwrap().id, "fleet");
        assert_eq!(spy_tier(3.0, &cfg).unwrap().id, "defences");
        assert_eq!(spy_tier(4.0, &cfg).unwrap().id, "garrison");
        assert!(reveals("garrison", "resources", &cfg));
        assert!(!reveals("fleet", "garrison", &cfg));
    }

    #[test]
    fn same_seed_repeats_detection_and_losses() {
        let cfg = EspionageConfig::default();
        let mut a = 7;
        let mut b = 7;
        let first = resolve_spy(4, 1.0, &mut a, &cfg);
        let second = resolve_spy(4, 1.0, &mut b, &cfg);
        assert_eq!(first, second);
        assert!(first.probes_lost <= 4);
        if !first.detected {
            assert_eq!(first.probes_lost, 0);
        }
    }
}
