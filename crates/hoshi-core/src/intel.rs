//! Delayed, noisy, and conflicting remote facts with provenance.
//!
//! A probe snapshot is an Observation. Facts keep every arrived observation
//! for the same object and field; a newer report does not overwrite an older
//! one. Light delay uses the difference of heliocentric radii at `c`.

use crate::combat::next_u64;
use crate::espionage::EspionageConfig;
use crate::fleets::Coord;
use crate::orbits::{OrbitConfig, heliocentric_radius_m};
use crate::ships::{Fleet, ShipKind};
use crate::state::Resources;

/// Speed of light in metres per second.
pub const SPEED_OF_LIGHT_MPS: f64 = 299_792_458.0;

/// One reading of one field on one remote object.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub report_id: u64,
    pub object: String,
    pub field: String,
    pub source: String,
    pub observed_at: u64,
    pub arrives_at: u64,
    pub age_secs: u64,
    pub confidence: f64,
    pub detected: bool,
    pub value: IntelValue,
}

/// Player-visible value. Never the hidden true state.
#[derive(Debug, Clone, PartialEq)]
pub enum IntelValue {
    Count(u32),
    Number(f64),
    Text(String),
}

/// All arrived observations for one object field, in report order.
#[derive(Debug, Clone, PartialEq)]
pub struct IntelFact {
    pub object: String,
    pub field: String,
    pub observations: Vec<Observation>,
}

/// Radial range in AU between two map slots.
pub fn light_range_au(from: Coord, to: Coord, orbits: &OrbitConfig) -> f64 {
    let a = heliocentric_radius_m(from, orbits);
    let b = heliocentric_radius_m(to, orbits);
    (a - b).abs() / orbits.au_metres.max(1.0)
}

/// Light-time in whole seconds for the radial range at `c`.
pub fn light_delay_secs(from: Coord, to: Coord, orbits: &OrbitConfig) -> u64 {
    let metres = light_range_au(from, to, orbits) * orbits.au_metres;
    (metres / SPEED_OF_LIGHT_MPS).round().max(0.0) as u64
}

/// Measurement error fraction from range and sensor quality.
pub fn sensor_error(range_au: f64, quality: f64, cfg: &EspionageConfig) -> f64 {
    cfg.error_base + cfg.error_per_au * range_au / quality.max(0.1)
}

/// Confidence in `[0.05, 1]`. Detection halves it; age spends `age_penalty_per_hour`.
pub fn intel_confidence(error: f64, age_secs: u64, detected: bool, cfg: &EspionageConfig) -> f64 {
    let age_hours = age_secs as f64 / 3600.0;
    let mut c = 1.0 - error - age_hours * cfg.age_penalty_per_hour;
    if detected {
        c *= 0.5;
    }
    c.clamp(0.05, 1.0)
}

/// Age the confidence stored at observation time. Later sensor upgrades do not rewrite it.
pub fn aged_confidence(stored: f64, age_secs: u64, cfg: &EspionageConfig) -> f64 {
    let age_hours = age_secs as f64 / 3600.0;
    (stored - age_hours * cfg.age_penalty_per_hour).clamp(0.05, 1.0)
}

/// Apply range error and optional decoy bias to a count.
pub fn apply_count_error(
    true_n: u32,
    error: f64,
    unit: f64,
    deceived: bool,
    decoy_bias: f64,
) -> u32 {
    if error == 0.0 && !deceived {
        return true_n;
    }
    let frac = error * unit.clamp(-1.0, 1.0) + if deceived { decoy_bias } else { 0.0 };
    ((f64::from(true_n)) * (1.0 + frac)).round().max(0.0) as u32
}

/// Apply range error and optional decoy bias to a stockpile amount.
pub fn apply_amount_error(
    true_v: f64,
    error: f64,
    unit: f64,
    deceived: bool,
    decoy_bias: f64,
) -> f64 {
    if error == 0.0 && !deceived {
        return true_v;
    }
    let frac = error * unit.clamp(-1.0, 1.0) + if deceived { decoy_bias } else { 0.0 };
    (true_v * (1.0 + frac)).max(0.0)
}

impl crate::state::GameState {
    /// Grouped remote facts visible to `actor` at `now`. Conflicting readings stay side by side.
    pub fn intel_facts_for(&self, actor: crate::leaders::Actor, now: u64) -> Vec<IntelFact> {
        let mut rows = Vec::new();
        for report in self.visible_reports(actor) {
            let confidence = aged_confidence(
                report.confidence,
                report.age_secs(now),
                &self.config.espionage,
            );
            for mut obs in observations_from(report, now) {
                obs.confidence = confidence;
                rows.push(obs);
            }
        }
        group_facts(rows)
    }

    pub fn provenance_for(
        &self,
        actor: crate::leaders::Actor,
        object: &str,
        field: &str,
        now: u64,
    ) -> Vec<Observation> {
        self.intel_facts_for(actor, now)
            .into_iter()
            .find(|fact| fact.object == object && fact.field == field)
            .map(|fact| fact.observations)
            .unwrap_or_default()
    }

    pub fn report_confidence(&self, report: &crate::espionage::SpyReport) -> f64 {
        aged_confidence(
            report.confidence,
            report.age_secs(self.last_tick_unix),
            &self.config.espionage,
        )
    }
}

pub fn signed_unit(rng: &mut u64) -> f64 {
    let u = (next_u64(rng) as f64) / ((u64::MAX as f64) + 1.0);
    2.0 * u - 1.0
}

pub fn noisy_resources(
    resources: Resources,
    error: f64,
    deceived: bool,
    decoy_bias: f64,
    rng: &mut u64,
) -> Resources {
    if error == 0.0 && !deceived {
        return resources;
    }
    Resources {
        iron: apply_amount_error(
            resources.iron,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
        silicon: apply_amount_error(
            resources.silicon,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
        rare_earths: apply_amount_error(
            resources.rare_earths,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
        water_ice: apply_amount_error(
            resources.water_ice,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
        helium_3: apply_amount_error(
            resources.helium_3,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
        deuterium: apply_amount_error(
            resources.deuterium,
            error,
            signed_unit(rng),
            deceived,
            decoy_bias,
        ),
    }
}

/// Flatten one delivered spy report into per-field observations.
pub fn observations_from(report: &crate::espionage::SpyReport, now: u64) -> Vec<Observation> {
    if !report.delivered(now) {
        return Vec::new();
    }
    let object = report.target.to_string();
    let mut out = Vec::new();
    let push = |out: &mut Vec<Observation>, field: &str, value: IntelValue| {
        out.push(Observation {
            report_id: report.id,
            object: object.clone(),
            field: field.into(),
            source: report.source.clone(),
            observed_at: report.observed_at,
            arrives_at: report.arrives_at,
            age_secs: report.age_secs(now),
            confidence: report.confidence,
            detected: report.detected,
            value,
        });
    };
    if let Some(kind) = report.kind {
        push(&mut out, "kind", IntelValue::Text(kind.as_key().into()));
    }
    if let Some(size) = report.size {
        push(&mut out, "size", IntelValue::Count(u32::from(size)));
    }
    if let Some(res) = report.resources {
        push(&mut out, "resources.iron", IntelValue::Number(res.iron));
        push(
            &mut out,
            "resources.silicon",
            IntelValue::Number(res.silicon),
        );
        push(
            &mut out,
            "resources.rare_earths",
            IntelValue::Number(res.rare_earths),
        );
        push(
            &mut out,
            "resources.water_ice",
            IntelValue::Number(res.water_ice),
        );
        push(
            &mut out,
            "resources.helium_3",
            IntelValue::Number(res.helium_3),
        );
        push(
            &mut out,
            "resources.deuterium",
            IntelValue::Number(res.deuterium),
        );
    }
    if let Some(fleet) = report.fleet {
        for kind in ShipKind::ALL {
            push(
                &mut out,
                &format!("fleet.{}", kind.as_key()),
                IntelValue::Count(fleet.get(kind)),
            );
        }
    }
    if let Some(defences) = report.defences {
        for kind in ShipKind::ALL {
            push(
                &mut out,
                &format!("defences.{}", kind.as_key()),
                IntelValue::Count(defences.get(kind)),
            );
        }
    }
    if let Some(garrison) = report.garrison {
        push(&mut out, "garrison", IntelValue::Count(garrison));
    }
    if let Some(formation) = &report.formation {
        push(&mut out, "formation", IntelValue::Text(formation.clone()));
    }
    if let Some(stance) = &report.stance {
        push(&mut out, "stance", IntelValue::Text(stance.clone()));
    }
    out
}

pub fn group_facts(observations: Vec<Observation>) -> Vec<IntelFact> {
    let mut grouped: std::collections::BTreeMap<(String, String), Vec<Observation>> =
        std::collections::BTreeMap::new();
    for row in observations {
        grouped
            .entry((row.object.clone(), row.field.clone()))
            .or_default()
            .push(row);
    }
    grouped
        .into_iter()
        .map(|((object, field), observations)| IntelFact {
            object,
            field,
            observations,
        })
        .collect()
}

pub fn noisy_fleet(
    fleet: Fleet,
    error: f64,
    deceived: bool,
    decoy_bias: f64,
    rng: &mut u64,
) -> Fleet {
    if error == 0.0 && !deceived {
        return fleet;
    }
    let mut out = Fleet::default();
    for kind in ShipKind::ALL {
        let n = fleet.get(kind);
        if n > 0 {
            out.add(
                kind,
                apply_count_error(n, error, signed_unit(rng), deceived, decoy_bias),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::fleets::Coord;

    fn orbits() -> OrbitConfig {
        BalanceConfig::default().orbits
    }

    #[test]
    fn light_delay_matches_distance_over_c() {
        let cfg = orbits();
        let home = Coord::HOME;
        let near = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let range = light_range_au(home, near, &cfg);
        let expected = (range * cfg.au_metres / SPEED_OF_LIGHT_MPS).round() as u64;
        assert!((range - 0.1).abs() < 1e-12, "{range}");
        assert_eq!(light_delay_secs(home, near, &cfg), expected);
        assert_eq!(light_delay_secs(home, home, &cfg), 0);
        let far = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        assert!(light_delay_secs(home, far, &cfg) > light_delay_secs(home, near, &cfg));
    }

    #[test]
    fn sensor_error_grows_with_range_and_falls_with_quality() {
        let cfg = EspionageConfig::default();
        let close = sensor_error(0.1, 1.0, &cfg);
        let far = sensor_error(10.0, 1.0, &cfg);
        let sharp = sensor_error(10.0, 5.0, &cfg);
        assert!((close - (cfg.error_base + cfg.error_per_au * 0.1)).abs() < 1e-12);
        assert!(far > close);
        assert!(sharp < far);
    }

    #[test]
    fn confidence_drops_with_error_age_and_detection() {
        let cfg = EspionageConfig::default();
        let fresh = intel_confidence(0.0, 0, false, &cfg);
        let noisy = intel_confidence(0.2, 0, false, &cfg);
        let old = intel_confidence(0.0, 3600, false, &cfg);
        let seen = intel_confidence(0.0, 0, true, &cfg);
        assert!((fresh - 1.0).abs() < 1e-12);
        assert!(noisy < fresh);
        assert!(old < fresh);
        assert!((seen - 0.5).abs() < 1e-12);
    }

    #[test]
    fn zero_error_without_deception_keeps_the_true_count() {
        assert_eq!(apply_count_error(40, 0.0, 0.7, false, 0.35), 40);
        assert_ne!(apply_count_error(40, 0.2, 1.0, false, 0.35), 40);
        assert_ne!(apply_count_error(40, 0.0, 0.0, true, 0.35), 40);
    }

    fn probe_report(
        id: u64,
        fleet: Option<crate::ships::Fleet>,
        confidence: f64,
    ) -> crate::espionage::SpyReport {
        crate::espionage::SpyReport {
            id,
            target: Coord::HOME,
            created_at: 0,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 2.0,
            tier: "fleet".into(),
            resources: None,
            fleet,
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 0,
            arrives_at: 0,
            confidence,
            deceived: false,
        }
    }

    #[test]
    fn aged_confidence_spends_the_hourly_penalty() {
        let cfg = EspionageConfig::default();
        let fresh = aged_confidence(0.8, 0, &cfg);
        let hour = aged_confidence(0.8, 3600, &cfg);
        assert!((fresh - 0.8).abs() < 1e-12);
        assert!((hour - (0.8 - cfg.age_penalty_per_hour)).abs() < 1e-12);
    }

    #[test]
    fn stored_confidence_does_not_rise_after_later_research() {
        let mut state = crate::state::GameState::new(3600);
        state.spy_reports.push(probe_report(1, None, 0.8));
        let before = state.report_confidence(&state.spy_reports[0]);
        state.tech_levels.insert("espionage".into(), 12);
        let after = state.report_confidence(&state.spy_reports[0]);
        assert_eq!(before, after);
        assert!((before - (0.8 - state.config.espionage.age_penalty_per_hour)).abs() < 1e-12);
    }

    #[test]
    fn revealed_empty_fleet_keeps_a_zero_count() {
        use crate::ships::{Fleet, ShipKind};
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 2);
        let first = observations_from(&probe_report(1, Some(ships), 1.0), 10);
        let later = observations_from(&probe_report(2, Some(Fleet::default()), 1.0), 10);
        let field = "fleet.light_fighter";
        assert_eq!(
            first.iter().find(|o| o.field == field).map(|o| &o.value),
            Some(&IntelValue::Count(2))
        );
        assert_eq!(
            later.iter().find(|o| o.field == field).map(|o| &o.value),
            Some(&IntelValue::Count(0))
        );
    }
}
