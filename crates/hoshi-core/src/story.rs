//! Data-driven story events. Adding a row in `story.toml` needs no code change.

use crate::ships::ShipKind;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

const DEFAULT_STORY: &str = include_str!("../data/story.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoryError {
    Parse(String),
    DuplicateId(String),
    EmptyChoices(String),
    DuplicateChoice { event: String, choice: String },
    UnknownTimeout { event: String, choice: String },
    UnknownShip { event: String, ship: String },
    BadWeight(String),
    BadTimeout(String),
    NoPending,
    UnknownChoice(String),
}

impl fmt::Display for StoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoryError::Parse(msg) => write!(f, "story: {msg}"),
            StoryError::DuplicateId(id) => write!(f, "story: duplicate id {id}"),
            StoryError::EmptyChoices(id) => write!(f, "story: {id} has no choices"),
            StoryError::DuplicateChoice { event, choice } => {
                write!(f, "story: {event} duplicate choice {choice}")
            }
            StoryError::UnknownTimeout { event, choice } => {
                write!(f, "story: {event} timeout_choice {choice} is unreachable")
            }
            StoryError::UnknownShip { event, ship } => {
                write!(f, "story: {event} unknown ship {ship}")
            }
            StoryError::BadWeight(id) => write!(f, "story: {id} weight must be positive"),
            StoryError::BadTimeout(id) => write!(f, "story: {id} timeout_secs must be positive"),
            StoryError::NoPending => write!(f, "story: no pending event"),
            StoryError::UnknownChoice(id) => write!(f, "story: unknown choice {id}"),
        }
    }
}

impl std::error::Error for StoryError {}

/// One offered event waiting for a player choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStory {
    pub event_id: String,
    pub offered_at: u64,
    pub timeout_at: u64,
}

/// Resource, ship, and raid effects for one choice. Unknown TOML keys are rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryChoice {
    pub id: String,
    pub label: String,
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub ships: Vec<(ShipKind, i32)>,
    pub schedule_raid: bool,
    pub metal_price: f64,
    pub crystal_price: f64,
    pub deuterium_price: f64,
    pub standing: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoryEvent {
    pub id: String,
    pub title: String,
    pub text: String,
    pub weight: u32,
    pub timeout_secs: u64,
    pub timeout_choice: String,
    pub min_elapsed_secs: u64,
    pub min_fighters: u32,
    pub choices: Vec<StoryChoice>,
}

impl StoryEvent {
    pub fn choice(&self, id: &str) -> Option<&StoryChoice> {
        self.choices.iter().find(|c| c.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoryCatalog {
    pub first_delay_secs: u64,
    pub cooldown_secs: u64,
    events: Vec<StoryEvent>,
}

impl StoryCatalog {
    pub fn parse(toml: &str) -> Result<Self, StoryError> {
        let file: File = toml::from_str(toml).map_err(|e| StoryError::Parse(e.to_string()))?;
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        for raw in file.event {
            if !seen.insert(raw.id.clone()) {
                return Err(StoryError::DuplicateId(raw.id));
            }
            events.push(raw.into_event()?);
        }
        Ok(Self {
            first_delay_secs: file.first_delay_secs,
            cooldown_secs: file.cooldown_secs,
            events,
        })
    }

    pub fn get(&self, id: &str) -> Option<&StoryEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &StoryEvent> {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// No events. Tests that must not consume RNG on the story path use this.
    pub fn empty() -> Self {
        Self {
            first_delay_secs: 1,
            cooldown_secs: 1,
            events: Vec::new(),
        }
    }
}

impl Default for StoryCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_STORY).expect("bundled story.toml must be valid")
    }
}

#[derive(Deserialize)]
struct File {
    first_delay_secs: u64,
    cooldown_secs: u64,
    #[serde(default)]
    event: Vec<FileEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEvent {
    id: String,
    title: String,
    text: String,
    weight: u32,
    timeout_secs: u64,
    timeout_choice: String,
    #[serde(default)]
    min_elapsed_secs: u64,
    #[serde(default)]
    min_fighters: u32,
    #[serde(default)]
    choices: Vec<FileChoice>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChoice {
    id: String,
    label: String,
    #[serde(default, alias = "metal")]
    iron: f64,
    #[serde(default, alias = "crystal")]
    silicon: f64,
    #[serde(default)]
    deuterium: f64,
    #[serde(default)]
    ships: BTreeMap<String, i32>,
    #[serde(default)]
    schedule_raid: bool,
    #[serde(default)]
    metal_price: f64,
    #[serde(default)]
    crystal_price: f64,
    #[serde(default)]
    deuterium_price: f64,
    #[serde(default)]
    standing: BTreeMap<String, f64>,
}

impl FileEvent {
    fn into_event(self) -> Result<StoryEvent, StoryError> {
        if self.choices.is_empty() {
            return Err(StoryError::EmptyChoices(self.id));
        }
        if self.weight == 0 {
            return Err(StoryError::BadWeight(self.id));
        }
        if self.timeout_secs == 0 {
            return Err(StoryError::BadTimeout(self.id));
        }
        let mut choice_ids = HashSet::new();
        let mut choices = Vec::new();
        for raw in self.choices {
            if !choice_ids.insert(raw.id.clone()) {
                return Err(StoryError::DuplicateChoice {
                    event: self.id,
                    choice: raw.id,
                });
            }
            let mut ships = Vec::new();
            for (key, count) in raw.ships {
                let kind = ShipKind::from_key(&key).ok_or_else(|| StoryError::UnknownShip {
                    event: self.id.clone(),
                    ship: key,
                })?;
                ships.push((kind, count));
            }
            choices.push(StoryChoice {
                id: raw.id,
                label: raw.label,
                iron: raw.iron,
                silicon: raw.silicon,
                deuterium: raw.deuterium,
                ships,
                schedule_raid: raw.schedule_raid,
                metal_price: raw.metal_price,
                crystal_price: raw.crystal_price,
                deuterium_price: raw.deuterium_price,
                standing: raw.standing,
            });
        }
        if !choice_ids.contains(&self.timeout_choice) {
            return Err(StoryError::UnknownTimeout {
                event: self.id,
                choice: self.timeout_choice,
            });
        }
        Ok(StoryEvent {
            id: self.id,
            title: self.title,
            text: self.text,
            weight: self.weight,
            timeout_secs: self.timeout_secs,
            timeout_choice: self.timeout_choice,
            min_elapsed_secs: self.min_elapsed_secs,
            min_fighters: self.min_fighters,
            choices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_at_least_ten_events() {
        let cat = StoryCatalog::default();
        assert!(cat.len() >= 10, "{}", cat.len());
        for event in cat.iter() {
            assert!(event.choice(&event.timeout_choice).is_some());
            assert!(!event.choices.is_empty());
            assert!(
                !event.id.starts_with("draft_"),
                "shipped story.toml must not include author drafts ({})",
                event.id
            );
        }
    }

    #[test]
    fn loader_rejects_unknown_effects_and_unreachable_timeout() {
        let unknown = r#"
first_delay_secs = 1
cooldown_secs = 1
[[event]]
id = "x"
title = "x"
text = "x"
weight = 1
timeout_secs = 1
timeout_choice = "a"
[[event.choices]]
id = "a"
label = "a"
magic = 1
"#;
        assert!(matches!(
            StoryCatalog::parse(unknown),
            Err(StoryError::Parse(_))
        ));

        let missing = r#"
first_delay_secs = 1
cooldown_secs = 1
[[event]]
id = "x"
title = "x"
text = "x"
weight = 1
timeout_secs = 1
timeout_choice = "gone"
[[event.choices]]
id = "a"
label = "a"
"#;
        assert!(matches!(
            StoryCatalog::parse(missing),
            Err(StoryError::UnknownTimeout { .. })
        ));
    }
}
