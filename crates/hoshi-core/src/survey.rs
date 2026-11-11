//! Seeded survey outcomes. Weights and the per-distance loss rate live in balance
//! config; a stored mission seed makes a pending result survive save/load.

use crate::combat::next_u64;

/// One row of the survey event table. Integer weights so a 10k-run test has an
/// exact target. Zero total means clean: surveys behave as they did before
/// the outcome roll.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurveyWeights {
    pub clean: u32,
    pub partial: u32,
    pub find: u32,
}

impl SurveyWeights {
    pub fn total(self) -> u32 {
        self.clean
            .saturating_add(self.partial)
            .saturating_add(self.find)
    }

    pub fn pick(self, seed: &mut u64) -> SurveyEvent {
        let total = self.total();
        if total == 0 {
            return SurveyEvent::Clean;
        }
        let mut r = next_u64(seed) % u64::from(total);
        let steps: [(u32, SurveyEvent); 3] = [
            (self.clean, SurveyEvent::Clean),
            (self.partial, SurveyEvent::Partial),
            (self.find, SurveyEvent::Find),
        ];
        for (weight, kind) in steps {
            if r < u64::from(weight) {
                return kind;
            }
            r -= u64::from(weight);
        }
        SurveyEvent::Clean
    }
}

/// Event table result. Probe losses are rolled separately against distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurveyEvent {
    Clean,
    Partial,
    Find,
}

impl SurveyEvent {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Partial => "partial",
            Self::Find => "find",
        }
    }
}

/// Numbers for the event table, per-probe loss, partial width and find debris.
#[derive(Debug, Clone, PartialEq)]
pub struct SurveyConfig {
    pub weights: SurveyWeights,
    /// Chance a probe is lost per unit of flight distance. Abstract rate.
    pub loss_per_distance: f64,
    /// Positions a partial survey reads, always including the destination.
    pub partial_slots: u32,
    /// Debris mass a find leaves at the destination, recyclable.
    pub find_debris: f64,
}

impl Default for SurveyConfig {
    fn default() -> Self {
        Self {
            weights: SurveyWeights::default(),
            loss_per_distance: 0.0,
            partial_slots: 6,
            find_debris: 0.0,
        }
    }
}

/// One resolved survey. The same seed always yields the same result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveyRoll {
    pub probes_lost: u32,
    pub probes_left: u32,
    pub event: SurveyEvent,
    /// Positions a partial survey read. Empty for clean and find.
    pub slots: Vec<u16>,
}

/// Loss chance for one probe on a flight of `distance`. Clamped to `[0, 1]`.
pub fn probe_loss_chance(distance: f64, cfg: &SurveyConfig) -> f64 {
    (distance.max(0.0) * cfg.loss_per_distance.max(0.0)).clamp(0.0, 1.0)
}

/// Draw survivors, then one event, from `seed`.
pub fn roll_survey(
    seed: u64,
    probes: u32,
    distance: f64,
    dest_position: u16,
    cfg: &SurveyConfig,
) -> SurveyRoll {
    let mut rng = seed;
    let threshold = (probe_loss_chance(distance, cfg) * 1_000_000.0).round() as u64;
    let mut lost = 0u32;
    for _ in 0..probes {
        if next_u64(&mut rng) % 1_000_000 < threshold {
            lost = lost.saturating_add(1);
        }
    }
    let left = probes.saturating_sub(lost);
    if left == 0 {
        return SurveyRoll {
            probes_lost: lost,
            probes_left: 0,
            event: SurveyEvent::Clean,
            slots: Vec::new(),
        };
    }
    let event = cfg.weights.pick(&mut rng);
    let slots = if event == SurveyEvent::Partial {
        pick_partial_slots(&mut rng, dest_position, cfg.partial_slots)
    } else {
        Vec::new()
    };
    SurveyRoll {
        probes_lost: lost,
        probes_left: left,
        event,
        slots,
    }
}

/// Destination plus more positions in `1..=16`, never the full system.
fn pick_partial_slots(seed: &mut u64, dest_position: u16, count: u32) -> Vec<u16> {
    let dest = dest_position.clamp(1, 16);
    let want = (count.max(1) as usize).min(15);
    let mut rest: Vec<u16> = (1..=16).filter(|&position| position != dest).collect();
    for i in (1..rest.len()).rev() {
        let j = (next_u64(seed) as usize) % (i + 1);
        rest.swap(i, j);
    }
    let mut slots = vec![dest];
    slots.extend(rest.into_iter().take(want.saturating_sub(1)));
    slots.sort_unstable();
    slots
}

fn probe_loss_clause(probes_lost: u32) -> Option<String> {
    match probes_lost {
        0 => None,
        1 => Some("a probe was lost".into()),
        n => Some(format!("{n} probes were lost")),
    }
}

/// Plain event line for the TUI result feed. No internal identifiers.
pub fn survey_event_text(
    system: &str,
    event: SurveyEvent,
    probes_lost: u32,
    probes_left: u32,
) -> String {
    if probes_left == 0 {
        return format!("probes lost surveying system {system}");
    }
    let base = match event {
        SurveyEvent::Partial => format!("partial survey of system {system}"),
        SurveyEvent::Find => format!("surveyed system {system}; found debris"),
        SurveyEvent::Clean => format!("surveyed system {system}"),
    };
    match probe_loss_clause(probes_lost) {
        Some(loss) => format!("{base}; {loss}"),
        None => base,
    }
}

/// History `outcome` in plain words. No internal identifiers.
pub fn survey_history_outcome(event: SurveyEvent, probes_lost: u32, probes_left: u32) -> String {
    if probes_left == 0 {
        return "probes lost".into();
    }
    let base = match event {
        SurveyEvent::Partial => "partial survey",
        SurveyEvent::Find => "found debris",
        SurveyEvent::Clean => "surveyed",
    };
    match probe_loss_clause(probes_lost) {
        Some(loss) => format!("{base}; {loss}"),
        None => base.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_replays_the_same_outcome() {
        let cfg = SurveyConfig {
            weights: SurveyWeights {
                clean: 80,
                partial: 12,
                find: 8,
            },
            loss_per_distance: 0.0001,
            partial_slots: 6,
            find_debris: 800.0,
        };
        let a = roll_survey(42, 3, 4_000.0, 8, &cfg);
        let b = roll_survey(42, 3, 4_000.0, 8, &cfg);
        assert_eq!(a, b);
    }

    #[test]
    fn zero_weights_and_loss_are_a_clean_survey() {
        let rolled = roll_survey(7, 1, 9_000.0, 4, &SurveyConfig::default());
        assert_eq!(rolled.probes_lost, 0);
        assert_eq!(rolled.probes_left, 1);
        assert_eq!(rolled.event, SurveyEvent::Clean);
        assert!(rolled.slots.is_empty());
    }

    #[test]
    fn far_surveys_lose_probes_more_often_than_near_ones() {
        let cfg = SurveyConfig {
            loss_per_distance: 0.0001,
            ..SurveyConfig::default()
        };
        let near_d = 1_075.0;
        let far_d = 4_125.0;
        let n = 10_000u32;
        let mut near_lost = 0u32;
        let mut far_lost = 0u32;
        for i in 0..n {
            near_lost += roll_survey(u64::from(i), 1, near_d, 8, &cfg).probes_lost;
            far_lost += roll_survey(u64::from(i) + 50_000, 1, far_d, 8, &cfg).probes_lost;
        }
        let near_rate = f64::from(near_lost) / f64::from(n);
        let far_rate = f64::from(far_lost) / f64::from(n);
        let near_p = probe_loss_chance(near_d, &cfg);
        let far_p = probe_loss_chance(far_d, &cfg);
        assert!(far_rate > near_rate, "far={far_rate} near={near_rate}");
        assert!(
            (near_rate - near_p).abs() < 0.02,
            "near {near_rate} vs {near_p}"
        );
        assert!((far_rate - far_p).abs() < 0.02, "far {far_rate} vs {far_p}");
    }

    #[test]
    fn partial_slots_include_the_destination_and_stay_short_of_a_system() {
        let mut seed = 11;
        let slots = pick_partial_slots(&mut seed, 5, 6);
        assert!(slots.contains(&5), "{slots:?}");
        assert_eq!(slots.len(), 6);
        assert!(slots.iter().all(|p| (1..=16).contains(p)), "{slots:?}");
        let mut seen = slots.clone();
        seen.dedup();
        assert_eq!(seen, slots);
    }

    #[test]
    fn event_and_history_lines_use_plain_words() {
        let text = survey_event_text("1:4", SurveyEvent::Partial, 0, 1);
        let outcome = survey_history_outcome(SurveyEvent::Find, 0, 1);
        assert_eq!(text, "partial survey of system 1:4");
        assert_eq!(outcome, "found debris");
        assert!(!text.contains('_'));
        assert!(!outcome.contains('_'));
        assert!(!text.contains("SurveyEvent"));
        assert!(!outcome.contains("SurveyEvent"));
        assert_eq!(
            survey_event_text("1:4", SurveyEvent::Partial, 1, 1),
            "partial survey of system 1:4; a probe was lost"
        );
        assert_eq!(
            survey_history_outcome(SurveyEvent::Find, 2, 1),
            "found debris; 2 probes were lost"
        );
        assert_eq!(
            survey_event_text("1:4", SurveyEvent::Clean, 2, 1),
            "surveyed system 1:4; 2 probes were lost"
        );
    }
}
