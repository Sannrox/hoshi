//! Opt-in self-play balance matrix. Never starts during normal play.

use crate::simulate::{DaySnapshot, VictorySimRow, simulate_victory_paths_for_seed};
use crate::{BalanceConfig, SpeciesCatalog};
use std::collections::BTreeMap;

/// Thresholds and seed lists for the CI self-play job.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfPlayConfig {
    /// Fail when a species or path is more than this fraction faster than the median.
    pub dominance_max: f64,
    /// Fail when one species takes more than this share of fastest finishes.
    /// Ignored when the seed list is shorter than 3.
    pub win_share_max: f64,
    pub pr_seeds: Vec<u64>,
    pub nightly_seeds: Vec<u64>,
    pub curve_days: u32,
}

impl Default for SelfPlayConfig {
    fn default() -> Self {
        Self {
            dominance_max: 0.25,
            win_share_max: 0.55,
            pr_seeds: vec![1],
            nightly_seeds: vec![1, 2, 3, 5, 8],
            curve_days: 3,
        }
    }
}

/// One species on one seed: path times plus a short resource curve.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfPlayCell {
    pub seed: u64,
    pub species: String,
    pub paths: Vec<VictorySimRow>,
    pub curve: Vec<DaySnapshot>,
}

/// Aggregated matrix used by the CLI and CI.
#[derive(Debug, Clone, PartialEq)]
pub struct SelfPlayReport {
    pub respec: Vec<crate::simulate::RespecRow>,
    pub seeds: Vec<u64>,
    pub cells: Vec<SelfPlayCell>,
    pub min_secs: u64,
    pub max_secs: u64,
    pub dominance_max: f64,
    pub win_share_max: f64,
}

impl SelfPlayReport {
    pub fn passed(&self) -> bool {
        self.pacing_ok() && self.time_dominance_ok() && self.win_share_ok()
    }

    pub fn pacing_ok(&self) -> bool {
        self.cells.iter().all(|cell| {
            cell.paths.iter().all(|row| {
                row.elapsed_secs
                    .is_some_and(|secs| secs >= self.min_secs && secs <= self.max_secs)
            })
        })
    }

    pub fn time_dominance_ok(&self) -> bool {
        for (_path, times) in self.times_by_path() {
            if times.is_empty() {
                return false;
            }
            let mut sorted = times;
            sorted.sort_unstable();
            let median = sorted[sorted.len() / 2];
            let floor = ((median as f64) * (1.0 - self.dominance_max)).round() as u64;
            if sorted.iter().any(|elapsed| *elapsed < floor) {
                return false;
            }
        }
        true
    }

    pub fn win_share_ok(&self) -> bool {
        if self.seeds.len() < 3 {
            return true;
        }
        let shares = self.win_shares();
        shares
            .values()
            .all(|share| *share <= self.win_share_max + 1e-9)
    }

    /// Fraction of (seed, path) cells where this species was fastest (ties split).
    pub fn win_shares(&self) -> BTreeMap<String, f64> {
        let mut wins: BTreeMap<String, f64> = BTreeMap::new();
        let mut cells = 0.0;
        for seed in &self.seeds {
            let group: Vec<_> = self.cells.iter().filter(|c| c.seed == *seed).collect();
            if group.is_empty() {
                continue;
            }
            let n_paths = group[0].paths.len();
            for i in 0..n_paths {
                let mut best = u64::MAX;
                let mut holders = Vec::new();
                for cell in &group {
                    let Some(elapsed) = cell.paths.get(i).and_then(|row| row.elapsed_secs) else {
                        continue;
                    };
                    if elapsed < best {
                        best = elapsed;
                        holders.clear();
                        holders.push(cell.species.clone());
                    } else if elapsed == best {
                        holders.push(cell.species.clone());
                    }
                }
                if holders.is_empty() {
                    continue;
                }
                cells += 1.0;
                let split = 1.0 / holders.len() as f64;
                for id in holders {
                    *wins.entry(id).or_insert(0.0) += split;
                }
            }
        }
        if cells > 0.0 {
            for share in wins.values_mut() {
                *share /= cells;
            }
        }
        wins
    }

    fn times_by_path(&self) -> BTreeMap<String, Vec<u64>> {
        let mut by_path = BTreeMap::new();
        for cell in &self.cells {
            for row in &cell.paths {
                if let Some(elapsed) = row.elapsed_secs {
                    by_path
                        .entry(row.path.clone())
                        .or_insert_with(Vec::new)
                        .push(elapsed);
                }
            }
        }
        by_path
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Self-play report\n\n");
        out.push_str(&crate::simulate::respec_markdown(&self.respec));
        out.push_str(&format!(
            "Seeds: {}\n",
            self.seeds
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push_str("Strategies: wonder, diplomacy, conquest. Every bundled species.\n");
        out.push_str(&format!(
            "Pacing band: {}s..={}s. Dominance max {:.0}%. Win-share max {:.0}% (seeds ≥ 3).\n\n",
            self.min_secs,
            self.max_secs,
            self.dominance_max * 100.0,
            self.win_share_max * 100.0
        ));
        out.push_str("## Win shares\n\n");
        out.push_str("| species | share |\n| --- | --- |\n");
        for (id, share) in self.win_shares() {
            out.push_str(&format!("| {id} | {share:.2} |\n"));
        }
        out.push_str(&format!(
            "\nWin-share check: {}\n\n",
            mark(self.win_share_ok())
        ));
        out.push_str("## Time to victory\n\n");
        out.push_str("| seed | species | path | elapsed | band |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for cell in &self.cells {
            for row in &cell.paths {
                let elapsed = match row.elapsed_secs {
                    Some(secs) => format!("{secs}s"),
                    None => "never".into(),
                };
                let in_band = row
                    .elapsed_secs
                    .is_some_and(|secs| secs >= self.min_secs && secs <= self.max_secs);
                out.push_str(&format!(
                    "| {} | {} | {} | {elapsed} | {} |\n",
                    cell.seed,
                    cell.species,
                    row.path,
                    mark(in_band)
                ));
            }
        }
        out.push_str(&format!(
            "\nPacing: {}  Time dominance: {}\n\n",
            mark(self.pacing_ok()),
            mark(self.time_dominance_ok())
        ));
        out.push_str("## Resource curves\n\n");
        out.push_str("| seed | species | day | iron | silicon | rare earths | water ice | helium-3 | deuterium |\n");
        out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for cell in &self.cells {
            for snap in &cell.curve {
                out.push_str(&format!(
                    "| {} | {} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |\n",
                    cell.seed,
                    cell.species,
                    snap.day,
                    snap.iron,
                    snap.silicon,
                    snap.rare_earths,
                    snap.water_ice,
                    snap.helium_3,
                    snap.deuterium
                ));
            }
        }
        out.push_str(&format!("\nOverall: {}\n", mark(self.passed())));
        out
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "pass" } else { "fail" }
}

/// Run the scripted victory strategies for each species on `seeds`.
pub fn run_self_play(config: BalanceConfig, seeds: &[u64]) -> SelfPlayReport {
    let respec = crate::simulate::simulate_respec(config.clone());
    let catalog = SpeciesCatalog::default();
    let min_secs = config.victory.casual_min_secs;
    let max_secs = config.victory.casual_max_secs;
    let dominance_max = config.self_play.dominance_max;
    let win_share_max = config.self_play.win_share_max;
    let curve_days = config.self_play.curve_days.max(1);
    let mut cells = Vec::new();
    let seeds: Vec<u64> = if seeds.is_empty() {
        vec![1]
    } else {
        seeds.to_vec()
    };
    for seed in &seeds {
        for spec in catalog.iter() {
            let paths =
                simulate_victory_paths_for_seed(config.clone(), Some(spec.id.as_str()), *seed);
            let curve = crate::simulate::greedy_curve(
                config.clone(),
                Some(spec.id.as_str()),
                *seed,
                curve_days,
            );
            cells.push(SelfPlayCell {
                seed: *seed,
                species: spec.id.clone(),
                paths,
                curve,
            });
        }
    }
    SelfPlayReport {
        respec,
        seeds,
        cells,
        min_secs,
        max_secs,
        dominance_max,
        win_share_max,
    }
}

pub fn path_bot_relpaths() -> [&'static str; 3] {
    [
        "examples/bots/wonder.sh",
        "examples/bots/diplomacy.sh",
        "examples/bots/conquest.sh",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_matrix_covers_every_species_and_path() {
        let config = BalanceConfig::default();
        let report = run_self_play(config.clone(), &config.self_play.pr_seeds);
        assert_eq!(report.cells.len(), 5);
        for cell in &report.cells {
            assert_eq!(cell.paths.len(), 3, "{}", cell.species);
            assert_eq!(cell.curve.len(), config.self_play.curve_days as usize);
        }
        assert!(report.passed(), "{}", report.to_markdown());
        assert!(report.to_markdown().contains("Self-play report"));
    }

    #[test]
    fn replay_seed_is_stable() {
        let config = BalanceConfig::default();
        let a = run_self_play(config.clone(), &[7]);
        let b = run_self_play(config, &[7]);
        assert_eq!(a, b);
    }

    #[test]
    fn seed_zero_replays_the_same_report() {
        let config = BalanceConfig::default();
        let a = run_self_play(config.clone(), &[0]);
        let b = run_self_play(config, &[0]);
        assert_eq!(a, b);
        assert!(a.cells.iter().all(|cell| cell.seed == 0));
    }
}
