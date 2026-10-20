//! Circular-orbit estimates for mission planning.
//!
//! Numbers are SI (metres, seconds, kilograms) then compressed into game
//! seconds by [`OrbitConfig::coast_compress`]. No n-body integration.

use crate::fleets::Coord;
use serde::Deserialize;
use std::f64::consts::{PI, TAU};

/// Bundled `[orbits]` table. Lengths are metres; `coast_compress` is game
/// seconds per real coast second.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitConfig {
    pub mu_star: f64,
    pub au_metres: f64,
    pub inner_au: f64,
    pub au_per_slot: f64,
    pub system_au_base: f64,
    pub system_au_step: f64,
    pub galaxy_au: f64,
    pub coast_compress: f64,
    pub window_half_secs: u64,
    pub window_count: u32,
    pub kg_per_hull: f64,
    pub kg_per_cargo: f64,
    pub m3_per_cargo: f64,
    pub combustion_ve: f64,
    pub impulse_ve: f64,
    pub hyperspace_ve: f64,
    pub well_rocky: f64,
    pub well_ice: f64,
    pub well_gas_giant: f64,
    pub high_energy_dv: f64,
    pub high_energy_time: f64,
    pub assist_dv: f64,
    pub turnaround_secs: u64,
}

impl Default for OrbitConfig {
    fn default() -> Self {
        FileOrbits::default()
            .into_config()
            .expect("bundled orbit defaults must be valid")
    }
}

impl OrbitConfig {
    /// Solar flux relative to 1 AU, using the same radii as mission planning.
    pub fn solar_flux(&self, coord: Coord) -> f64 {
        (self.au_metres / heliocentric_radius_m(coord, self)).powi(2)
    }

    pub fn ve_for_drive(&self, drive: &str) -> f64 {
        match drive {
            "impulse_drive" => self.impulse_ve,
            "hyperspace_drive" => self.hyperspace_ve,
            _ => self.combustion_ve,
        }
    }

    pub fn well_dv(&self, kind: crate::PlanetType) -> f64 {
        match kind {
            crate::PlanetType::Rocky => self.well_rocky,
            crate::PlanetType::Ice => self.well_ice,
            crate::PlanetType::GasGiant => self.well_gas_giant,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct FileOrbits {
    #[serde(default = "default_mu")]
    mu_star: f64,
    #[serde(default = "default_au")]
    au_metres: f64,
    #[serde(default = "default_inner_au")]
    inner_au: f64,
    #[serde(default = "default_au_per_slot")]
    au_per_slot: f64,
    #[serde(default = "default_system_base")]
    system_au_base: f64,
    #[serde(default = "default_system_step")]
    system_au_step: f64,
    #[serde(default = "default_galaxy_au")]
    galaxy_au: f64,
    #[serde(default = "default_compress")]
    coast_compress: f64,
    #[serde(default = "default_window_half")]
    window_half_secs: u64,
    #[serde(default = "default_window_count")]
    window_count: u32,
    #[serde(default = "default_kg_hull")]
    kg_per_hull: f64,
    #[serde(default = "default_kg_cargo")]
    kg_per_cargo: f64,
    #[serde(default = "default_m3")]
    m3_per_cargo: f64,
    #[serde(default = "default_combustion")]
    combustion_ve: f64,
    #[serde(default = "default_impulse")]
    impulse_ve: f64,
    #[serde(default = "default_hyperspace")]
    hyperspace_ve: f64,
    #[serde(default = "default_well_rocky")]
    well_rocky: f64,
    #[serde(default = "default_well_ice")]
    well_ice: f64,
    #[serde(default = "default_well_gas")]
    well_gas_giant: f64,
    #[serde(default = "default_he_dv")]
    high_energy_dv: f64,
    #[serde(default = "default_he_time")]
    high_energy_time: f64,
    #[serde(default = "default_assist")]
    assist_dv: f64,
    #[serde(default = "default_turnaround")]
    turnaround_secs: u64,
}

impl Default for FileOrbits {
    fn default() -> Self {
        Self {
            mu_star: default_mu(),
            au_metres: default_au(),
            inner_au: default_inner_au(),
            au_per_slot: default_au_per_slot(),
            system_au_base: default_system_base(),
            system_au_step: default_system_step(),
            galaxy_au: default_galaxy_au(),
            coast_compress: default_compress(),
            window_half_secs: default_window_half(),
            window_count: default_window_count(),
            kg_per_hull: default_kg_hull(),
            kg_per_cargo: default_kg_cargo(),
            m3_per_cargo: default_m3(),
            combustion_ve: default_combustion(),
            impulse_ve: default_impulse(),
            hyperspace_ve: default_hyperspace(),
            well_rocky: default_well_rocky(),
            well_ice: default_well_ice(),
            well_gas_giant: default_well_gas(),
            high_energy_dv: default_he_dv(),
            high_energy_time: default_he_time(),
            assist_dv: default_assist(),
            turnaround_secs: default_turnaround(),
        }
    }
}

impl FileOrbits {
    pub(crate) fn into_config(self) -> Result<OrbitConfig, String> {
        for (name, value) in [
            ("mu_star", self.mu_star),
            ("au_metres", self.au_metres),
            ("inner_au", self.inner_au),
            ("au_per_slot", self.au_per_slot),
            ("system_au_base", self.system_au_base),
            ("system_au_step", self.system_au_step),
            ("galaxy_au", self.galaxy_au),
            ("coast_compress", self.coast_compress),
            ("kg_per_hull", self.kg_per_hull),
            ("kg_per_cargo", self.kg_per_cargo),
            ("m3_per_cargo", self.m3_per_cargo),
            ("combustion_ve", self.combustion_ve),
            ("impulse_ve", self.impulse_ve),
            ("hyperspace_ve", self.hyperspace_ve),
            ("well_rocky", self.well_rocky),
            ("well_ice", self.well_ice),
            ("well_gas_giant", self.well_gas_giant),
            ("high_energy_dv", self.high_energy_dv),
            ("high_energy_time", self.high_energy_time),
            ("assist_dv", self.assist_dv),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(format!("orbits.{name} must be > 0"));
            }
        }
        if self.window_count == 0 {
            return Err("orbits.window_count must be >= 1".into());
        }
        if self.high_energy_dv < 1.0 {
            return Err("orbits.high_energy_dv must be >= 1".into());
        }
        if self.high_energy_time >= 1.0 {
            return Err("orbits.high_energy_time must be < 1".into());
        }
        if self.assist_dv >= 1.0 {
            return Err("orbits.assist_dv must be < 1".into());
        }
        Ok(OrbitConfig {
            mu_star: self.mu_star,
            au_metres: self.au_metres,
            inner_au: self.inner_au,
            au_per_slot: self.au_per_slot,
            system_au_base: self.system_au_base,
            system_au_step: self.system_au_step,
            galaxy_au: self.galaxy_au,
            coast_compress: self.coast_compress,
            window_half_secs: self.window_half_secs.max(1),
            window_count: self.window_count,
            kg_per_hull: self.kg_per_hull,
            kg_per_cargo: self.kg_per_cargo,
            m3_per_cargo: self.m3_per_cargo,
            combustion_ve: self.combustion_ve,
            impulse_ve: self.impulse_ve,
            hyperspace_ve: self.hyperspace_ve,
            well_rocky: self.well_rocky,
            well_ice: self.well_ice,
            well_gas_giant: self.well_gas_giant,
            high_energy_dv: self.high_energy_dv,
            high_energy_time: self.high_energy_time,
            assist_dv: self.assist_dv,
            turnaround_secs: self.turnaround_secs.max(1),
        })
    }
}

fn default_mu() -> f64 {
    1.327_124_4e20
}
fn default_au() -> f64 {
    1.495_978_707e11
}
fn default_inner_au() -> f64 {
    1.0
}
fn default_au_per_slot() -> f64 {
    0.1
}
fn default_system_base() -> f64 {
    40.0
}
fn default_system_step() -> f64 {
    5.0
}
fn default_galaxy_au() -> f64 {
    1.0e5
}
fn default_compress() -> f64 {
    2.7e-5
}
fn default_window_half() -> u64 {
    120
}
fn default_window_count() -> u32 {
    4
}
fn default_kg_hull() -> f64 {
    2.0
}
fn default_kg_cargo() -> f64 {
    1.0
}
fn default_m3() -> f64 {
    1.0
}
fn default_combustion() -> f64 {
    3_100.0
}
fn default_impulse() -> f64 {
    8_000.0
}
fn default_hyperspace() -> f64 {
    30_000.0
}
fn default_well_rocky() -> f64 {
    800.0
}
fn default_well_ice() -> f64 {
    500.0
}
fn default_well_gas() -> f64 {
    1_500.0
}
fn default_he_dv() -> f64 {
    1.65
}
fn default_he_time() -> f64 {
    0.55
}
fn default_assist() -> f64 {
    0.82
}
fn default_turnaround() -> u64 {
    120
}

/// Two-burn Hohmann estimate between circular orbits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hohmann {
    pub delta_v: f64,
    pub coast_secs: f64,
    pub semi_major: f64,
}

/// Heliocentric radius in metres for a map slot.
///
/// Slot 1 of system 1 in galaxy 1 is `inner_au`. Later slots add
/// `au_per_slot`. Later systems add `system_au_base` then `system_au_step`.
/// Later galaxies add `galaxy_au`.
pub fn heliocentric_radius_m(coord: Coord, cfg: &OrbitConfig) -> f64 {
    let slot = f64::from(coord.position.saturating_sub(1));
    let system = coord.system.saturating_sub(1);
    let galaxy = f64::from(coord.galaxy.saturating_sub(1));
    let system_au = if system == 0 {
        0.0
    } else {
        cfg.system_au_base + f64::from(system - 1) * cfg.system_au_step
    };
    let au = cfg.inner_au + slot * cfg.au_per_slot + system_au + galaxy * cfg.galaxy_au;
    au * cfg.au_metres
}

pub fn mean_motion(mu: f64, radius_m: f64) -> f64 {
    (mu / radius_m.powi(3)).sqrt()
}

pub fn hohmann_transfer(r1: f64, r2: f64, mu: f64) -> Hohmann {
    let a = 0.5 * (r1 + r2);
    let v_c1 = (mu / r1).sqrt();
    let v_c2 = (mu / r2).sqrt();
    let v_t1 = (mu * (2.0 / r1 - 1.0 / a).max(0.0)).sqrt();
    let v_t2 = (mu * (2.0 / r2 - 1.0 / a).max(0.0)).sqrt();
    Hohmann {
        delta_v: (v_t1 - v_c1).abs() + (v_c2 - v_t2).abs(),
        coast_secs: PI * (a.powi(3) / mu).sqrt(),
        semi_major: a,
    }
}

/// `delta-v = ve * ln(m0 / mf)`.
pub fn rocket_delta_v(ve: f64, wet_kg: f64, dry_kg: f64) -> f64 {
    if ve <= 0.0 || dry_kg <= 0.0 || wet_kg <= dry_kg || !wet_kg.is_finite() {
        return 0.0;
    }
    ve * (wet_kg / dry_kg).ln()
}

/// Propellant mass so a dry stack can supply `delta_v`.
pub fn fuel_mass_for_dv(ve: f64, dry_kg: f64, delta_v: f64) -> f64 {
    if ve <= 0.0 || dry_kg <= 0.0 || delta_v <= 0.0 {
        return 0.0;
    }
    dry_kg * ((delta_v / ve).exp() - 1.0)
}

pub fn synodic_secs(n1: f64, n2: f64) -> Option<f64> {
    let dn = (n1 - n2).abs();
    if dn < 1e-18 { None } else { Some(TAU / dn) }
}

/// Real-second timestamps of departure windows for `coast_secs`, then compressed.
/// Phase uses a 180° Hohmann-like lead; high-energy passes its shortened coast.
pub fn launch_window_times(
    now: u64,
    n_origin: f64,
    n_dest: f64,
    coast_secs: f64,
    cfg: &OrbitConfig,
) -> Vec<(u64, u64)> {
    let half = cfg.window_half_secs;
    let count = cfg.window_count as usize;
    let compress = cfg.coast_compress;
    if let Some(period) = synodic_secs(n_origin, n_dest) {
        let phase = (PI - n_dest * coast_secs).rem_euclid(TAU);
        let n_rel = n_dest - n_origin;
        let mut t0 = if n_rel.abs() < 1e-18 {
            0.0
        } else {
            phase / n_rel
        };
        if !t0.is_finite() {
            t0 = 0.0;
        }
        t0 = t0.rem_euclid(period);
        let now_real = now as f64 / compress;
        let mut k = ((now_real - t0) / period).floor();
        if k.is_nan() || k.is_infinite() {
            k = 0.0;
        }
        let mut out = Vec::with_capacity(count);
        let mut n = 0;
        while out.len() < count && n < count + 8 {
            let real = t0 + (k + n as f64) * period;
            n += 1;
            if real < 0.0 {
                continue;
            }
            let centre = (real * compress).floor().max(0.0) as u64;
            let opens = centre.saturating_sub(half);
            let closes = centre.saturating_add(half);
            if closes <= now {
                continue;
            }
            out.push((opens, closes));
        }
        if out.is_empty() {
            let opens = now;
            out.push((opens, opens.saturating_add(half.saturating_mul(2))));
        }
        out
    } else {
        vec![(now, now.saturating_add(half.saturating_mul(2)))]
    }
}

pub fn to_game_secs(real_secs: f64, compress: f64) -> u64 {
    (real_secs * compress).ceil().max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hohmann_1_to_1_5_au_matches_known_coast_and_delta_v() {
        let cfg = OrbitConfig::default();
        let r1 = cfg.au_metres;
        let r2 = 1.5 * cfg.au_metres;
        let burn = hohmann_transfer(r1, r2, cfg.mu_star);
        let days = burn.coast_secs / 86_400.0;
        assert!(
            (days - 255.0).abs() < 3.0,
            "coast days {days} want ~255 (8–9 months)"
        );
        assert!(
            (burn.delta_v - 5_413.0).abs() < 80.0,
            "delta-v {} m/s want ~5.4 km/s",
            burn.delta_v
        );
    }

    #[test]
    fn rocket_equation_matches_ln_ratio() {
        let ve = 3_100.0;
        let wet = 1_000.0;
        let dry = 400.0;
        let dv = rocket_delta_v(ve, wet, dry);
        let expect = ve * (wet / dry).ln();
        assert!((dv - expect).abs() < 1e-9);
        assert!((dv - 2_840.5).abs() < 1.0, "{dv}");
        let fuel = fuel_mass_for_dv(ve, dry, dv);
        assert!((fuel - (wet - dry)).abs() < 1e-6);
    }

    #[test]
    fn extra_payload_cuts_available_delta_v_for_the_same_fuel() {
        let ve = 3_100.0;
        let dry = 400.0;
        let fuel = 600.0;
        let light = rocket_delta_v(ve, dry + fuel, dry);
        let heavy = rocket_delta_v(ve, dry + 200.0 + fuel, dry + 200.0);
        assert!(heavy < light);
    }

    #[test]
    fn earth_mars_synodic_is_hundreds_of_days() {
        let cfg = OrbitConfig::default();
        let n1 = mean_motion(cfg.mu_star, cfg.au_metres);
        let n2 = mean_motion(cfg.mu_star, 1.5 * cfg.au_metres);
        let days = synodic_secs(n1, n2).unwrap() / 86_400.0;
        assert!(
            (days - 803.0).abs() < 20.0,
            "synodic days {days} want ~800 at 1.5 AU"
        );
    }

    #[test]
    fn home_slot_sits_at_inner_au() {
        let cfg = OrbitConfig::default();
        let home = heliocentric_radius_m(Coord::HOME, &cfg) / cfg.au_metres;
        assert!((home - cfg.inner_au).abs() < 1e-12, "home {home} AU");
        let neighbor = heliocentric_radius_m(
            Coord {
                galaxy: 1,
                system: 1,
                position: 2,
            },
            &cfg,
        ) / cfg.au_metres;
        assert!((neighbor - (cfg.inner_au + cfg.au_per_slot)).abs() < 1e-12);
        let other_sys = heliocentric_radius_m(
            Coord {
                galaxy: 1,
                system: 2,
                position: 1,
            },
            &cfg,
        ) / cfg.au_metres;
        assert!((other_sys - (cfg.inner_au + cfg.system_au_base)).abs() < 1e-12);
        let mars = heliocentric_radius_m(
            Coord {
                galaxy: 1,
                system: 1,
                position: 6,
            },
            &cfg,
        ) / cfg.au_metres;
        assert!(
            (mars - 1.5).abs() < 1e-12,
            "bundled slot 6 should be 1.5 AU, got {mars}"
        );
    }

    #[test]
    fn inner_hops_have_shorter_coast_than_cross_system() {
        let cfg = OrbitConfig::default();
        let home = Coord::HOME;
        let neighbor = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let other_sys = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        let inner = hohmann_transfer(
            heliocentric_radius_m(home, &cfg),
            heliocentric_radius_m(neighbor, &cfg),
            cfg.mu_star,
        );
        let hop = hohmann_transfer(
            heliocentric_radius_m(home, &cfg),
            heliocentric_radius_m(other_sys, &cfg),
            cfg.mu_star,
        );
        assert!(inner.coast_secs < hop.coast_secs);
        assert!(inner.delta_v < hop.delta_v);
    }
}
