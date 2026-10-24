//! Asset condition, telemetry, work orders and failure prediction.
//!
//! Condition drops with use, age and heat overflow. A bathtub-style hourly
//! failure chance uses infant hours, a mid-life base rate, and wear-out below
//! `wearout_condition`. Telemetry is the true signal plus bounded noise from a
//! splitmix of (save seed, asset id, time). Predicted failure in the next
//! horizon is `1 - (1 - p)^hours` for that hourly chance.

use crate::buildings::BuildingKind;
use crate::defences::DefenceKind;
use crate::fleets::Coord;
use crate::history::HistoryKind;
use crate::leaders::Actor;
use crate::state::{GameState, Resources};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;

const DEFAULT: &str = include_str!("../data/maintenance.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum MaintenanceError {
    Parse(String),
    UnknownAsset,
    UnknownOrder,
    NotOwner,
    Busy,
    Unaffordable,
    NotDue,
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaintenanceError::Parse(msg) => write!(f, "maintenance: {msg}"),
            MaintenanceError::UnknownAsset => write!(f, "unknown asset"),
            MaintenanceError::UnknownOrder => write!(f, "unknown work order"),
            MaintenanceError::NotOwner => write!(f, "actor cannot schedule maintenance"),
            MaintenanceError::Busy => write!(f, "asset already has a work order"),
            MaintenanceError::Unaffordable => write!(f, "not enough parts for the work order"),
            MaintenanceError::NotDue => write!(f, "work order is still in progress"),
        }
    }
}

impl std::error::Error for MaintenanceError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MaintenanceConfig {
    pub telemetry_secs: u64,
    pub archive_cap: usize,
    pub horizon_secs: u64,
    pub noise: f64,
    pub infant_hours: f64,
    pub wearout_condition: f64,
    pub wear_use_per_hour: f64,
    pub wear_age_per_hour: f64,
    pub wear_heat_per_hour: f64,
    pub fail_base_per_hour: f64,
    pub fail_infant_per_hour: f64,
    pub fail_wearout_per_hour: f64,
    pub offline_secs: u64,
    pub work_secs: u64,
    pub iron_cost: f64,
    pub silicon_cost: f64,
    pub interval_secs: u64,
    pub interval_hours_use: f64,
}

impl MaintenanceConfig {
    pub fn bundled() -> Self {
        static BUNDLED: std::sync::OnceLock<MaintenanceConfig> = std::sync::OnceLock::new();
        BUNDLED
            .get_or_init(|| Self::parse(DEFAULT).expect("bundled maintenance.toml"))
            .clone()
    }

    pub fn parse(raw: &str) -> Result<Self, MaintenanceError> {
        let cfg: Self = toml::from_str(raw).map_err(|e| MaintenanceError::Parse(e.to_string()))?;
        if cfg.telemetry_secs == 0
            || cfg.archive_cap == 0
            || cfg.horizon_secs == 0
            || cfg.offline_secs == 0
            || cfg.work_secs == 0
            || cfg.interval_secs == 0
        {
            return Err(MaintenanceError::Parse("intervals must be > 0".into()));
        }
        if !(0.0..=0.5).contains(&cfg.noise) {
            return Err(MaintenanceError::Parse("noise must be in 0..=0.5".into()));
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Building,
    Defence,
    Vehicle,
}

impl AssetKind {
    pub fn as_key(self) -> &'static str {
        match self {
            AssetKind::Building => "building",
            AssetKind::Defence => "defence",
            AssetKind::Vehicle => "vehicle",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "building" => AssetKind::Building,
            "defence" => AssetKind::Defence,
            "vehicle" => AssetKind::Vehicle,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Time,
    Usage,
}

impl PlanKind {
    pub fn as_key(self) -> &'static str {
        match self {
            PlanKind::Time => "time",
            PlanKind::Usage => "usage",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "time" => PlanKind::Time,
            "usage" => PlanKind::Usage,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    Scheduled,
    Repair,
}

impl WorkKind {
    pub fn as_key(self) -> &'static str {
        match self {
            WorkKind::Scheduled => "scheduled",
            WorkKind::Repair => "repair",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "scheduled" => WorkKind::Scheduled,
            "repair" => WorkKind::Repair,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub id: u64,
    pub kind: AssetKind,
    pub key: String,
    pub coord: Coord,
    pub condition: f64,
    pub hours_use: f64,
    pub installed_at: u64,
    pub last_service_at: u64,
    pub last_service_hours: f64,
    pub offline_until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaintenancePlan {
    pub id: u64,
    pub asset_id: u64,
    pub kind: PlanKind,
    pub interval: f64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkOrder {
    pub id: u64,
    pub asset_id: u64,
    pub kind: WorkKind,
    pub started_at: u64,
    pub ends_at: u64,
    pub iron: f64,
    pub silicon: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryReading {
    pub id: u64,
    pub asset_id: u64,
    pub at: u64,
    pub temperature: f64,
    pub vibration: f64,
    pub power: f64,
    pub hours_use: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailurePrediction {
    pub id: u64,
    pub asset_id: u64,
    pub at: u64,
    pub probability: f64,
    pub horizon_secs: u64,
    pub reading_id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaintenanceState {
    pub next_id: u64,
    pub next_plan_id: u64,
    pub next_order_id: u64,
    pub next_reading_id: u64,
    pub next_prediction_id: u64,
    pub next_at: u64,
    pub assets: Vec<Asset>,
    pub plans: Vec<MaintenancePlan>,
    pub orders: Vec<WorkOrder>,
    pub readings: Vec<TelemetryReading>,
    pub predictions: Vec<FailurePrediction>,
}

impl MaintenanceState {
    pub fn new(now: u64, cfg: &MaintenanceConfig) -> Self {
        Self {
            next_id: 1,
            next_plan_id: 1,
            next_order_id: 1,
            next_reading_id: 1,
            next_prediction_id: 1,
            next_at: now.saturating_add(cfg.telemetry_secs),
            assets: Vec::new(),
            plans: Vec::new(),
            orders: Vec::new(),
            readings: Vec::new(),
            predictions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaintenanceSnapshot {
    pub assets: Vec<Asset>,
    pub plans: Vec<MaintenancePlan>,
    pub orders: Vec<WorkOrder>,
    pub readings: Vec<TelemetryReading>,
    pub predictions: Vec<FailurePrediction>,
    pub next_at: u64,
}

/// Hourly failure chance: infant mortality, mid-life base, wear-out, heat.
pub fn fail_p_per_hour(
    cfg: &MaintenanceConfig,
    hours_use: f64,
    condition: f64,
    heat_overflow: bool,
) -> f64 {
    let infant = if hours_use < cfg.infant_hours && cfg.infant_hours > 0.0 {
        cfg.fail_infant_per_hour * (1.0 - hours_use / cfg.infant_hours)
    } else {
        0.0
    };
    let wearout = if condition < cfg.wearout_condition && cfg.wearout_condition > 0.0 {
        cfg.fail_wearout_per_hour * (1.0 - condition / cfg.wearout_condition)
    } else {
        0.0
    };
    let heat = if heat_overflow { 1.5 } else { 1.0 };
    ((infant + cfg.fail_base_per_hour + wearout) * heat).clamp(0.0, 0.5)
}

/// `1 - (1 - p)^hours` over the configured horizon.
pub fn predict_failure(cfg: &MaintenanceConfig, p_hour: f64) -> f64 {
    let hours = cfg.horizon_secs as f64 / 3600.0;
    if p_hour <= 0.0 || hours <= 0.0 {
        return 0.0;
    }
    1.0 - (1.0 - p_hour).powf(hours)
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit_noise(seed: u64, asset_id: u64, at: u64) -> f64 {
    let x = splitmix(seed ^ asset_id.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ at);
    (x as f64 / u64::MAX as f64) * 2.0 - 1.0
}

fn output_factor(asset: &Asset, now: u64) -> f64 {
    if asset.offline_until.is_some_and(|until| until > now) {
        return 0.0;
    }
    (0.5 + 0.5 * asset.condition).clamp(0.0, 1.0)
}

impl GameState {
    /// Player-facing name for a maintenance asset: `metal mine at 1:1:1`,
    /// `light cannon at 1:1:1`, `vehicle 3`. The key stays the stable id.
    pub fn asset_label(&self, asset: &Asset) -> String {
        let id = asset.key.rsplit(':').next().unwrap_or(&asset.key);
        match asset.kind {
            AssetKind::Building => format!("{} at {}", self.display_name(id), asset.coord),
            AssetKind::Defence => {
                let name = crate::DefenceKind::from_key(id)
                    .map_or_else(|| id.replace('_', " "), |kind| kind.to_string());
                format!("{name} at {}", asset.coord)
            }
            AssetKind::Vehicle => format!("vehicle {id}"),
        }
    }

    pub fn maintenance_config(&self) -> MaintenanceConfig {
        MaintenanceConfig::bundled()
    }

    pub fn maintenance_snapshot(&self) -> MaintenanceSnapshot {
        MaintenanceSnapshot {
            assets: self.maintenance.assets.clone(),
            plans: self.maintenance.plans.clone(),
            orders: self.maintenance.orders.clone(),
            readings: self.maintenance.readings.clone(),
            predictions: self.maintenance.predictions.clone(),
            next_at: self.maintenance.next_at,
        }
    }

    pub fn telemetry_jsonl(&self) -> String {
        self.maintenance
            .readings
            .iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "asset_id": row.asset_id,
                    "at": row.at,
                    "temperature": row.temperature,
                    "vibration": row.vibration,
                    "power": row.power,
                    "hours_use": row.hours_use,
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn building_health(&self, coord: Coord, kind: BuildingKind) -> f64 {
        if self.dismantling(coord) == Some(kind) {
            return 0.0;
        }
        let key = format!("building:{}:{}", coord, kind.as_key());
        let Some(asset) = self.maintenance.assets.iter().find(|a| a.key == key) else {
            return 1.0;
        };
        if self
            .maintenance
            .orders
            .iter()
            .any(|order| order.asset_id == asset.id)
        {
            return 0.0;
        }
        output_factor(asset, self.last_tick_unix)
    }

    pub fn schedule_maintenance(
        &mut self,
        asset_id: u64,
        plan: PlanKind,
        actor: Actor,
        now_unix: u64,
    ) -> Result<u64, MaintenanceError> {
        self.catch_up(now_unix);
        if actor != Actor::Player {
            return Err(MaintenanceError::NotOwner);
        }
        let cfg = self.maintenance_config();
        let idx = self
            .maintenance
            .assets
            .iter()
            .position(|a| a.id == asset_id)
            .ok_or(MaintenanceError::UnknownAsset)?;
        if self
            .maintenance
            .orders
            .iter()
            .any(|o| o.asset_id == asset_id)
        {
            return Err(MaintenanceError::Busy);
        }
        let started = self.last_tick_unix;
        let id = self.open_work_order(asset_id, WorkKind::Scheduled, started, &cfg)?;
        let interval = match plan {
            PlanKind::Time => cfg.interval_secs as f64,
            PlanKind::Usage => cfg.interval_hours_use,
        };
        if let Some(row) = self
            .maintenance
            .plans
            .iter_mut()
            .find(|p| p.asset_id == asset_id)
        {
            row.kind = plan;
            row.interval = interval;
            row.enabled = true;
        } else {
            let plan_id = self.maintenance.next_plan_id;
            self.maintenance.next_plan_id = self.maintenance.next_plan_id.saturating_add(1);
            self.maintenance.plans.push(MaintenancePlan {
                id: plan_id,
                asset_id,
                kind: plan,
                interval,
                enabled: true,
            });
        }
        let key = self.maintenance.assets[idx].key.clone();
        self.record_history(crate::HistoryDraft {
            at: started,
            kind: HistoryKind::Other,
            actor: actor.as_key(),
            action: "schedule_maintenance".into(),
            objects: vec![format!("asset:{asset_id}"), key],
            outcome: format!("order:{id}"),
            ..crate::HistoryDraft::default()
        });
        Ok(id)
    }

    pub fn complete_work_order(
        &mut self,
        order_id: u64,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), MaintenanceError> {
        self.catch_up(now_unix);
        if actor != Actor::Player {
            return Err(MaintenanceError::NotOwner);
        }
        let at = self.last_tick_unix;
        let pos = self
            .maintenance
            .orders
            .iter()
            .position(|o| o.id == order_id);
        let Some(pos) = pos else {
            // Catch-up auto-finishes due orders; repeating complete is a no-op.
            if order_id > 0 && order_id < self.maintenance.next_order_id {
                return Ok(());
            }
            return Err(MaintenanceError::UnknownOrder);
        };
        if self.maintenance.orders[pos].ends_at > at {
            return Err(MaintenanceError::NotDue);
        }
        let order = self.maintenance.orders[pos].clone();
        if !self.can_pay_work_order(&order) {
            return Err(MaintenanceError::Unaffordable);
        }
        self.maintenance.orders.remove(pos);
        self.finish_order(order, at);
        Ok(())
    }

    pub(crate) fn next_maintenance_event(&self) -> Option<u64> {
        // Work-order completions only. Telemetry `next_at` is applied inside
        // `produce_until` so a 7-day catch-up does not grow a world-event per hour.
        self.maintenance
            .orders
            .iter()
            .map(|order| order.ends_at)
            .filter(|&at| at > self.last_tick_unix)
            .min()
    }

    pub fn apply_maintenance_at(&mut self, at: u64) {
        self.sync_maintenance_assets(at);
        let due: Vec<WorkOrder> = self
            .maintenance
            .orders
            .iter()
            .filter(|o| o.ends_at <= at)
            .cloned()
            .collect();
        for order in due {
            if !self.can_pay_work_order(&order) {
                if let Some(asset) = self
                    .maintenance
                    .assets
                    .iter_mut()
                    .find(|a| a.id == order.asset_id)
                {
                    asset.offline_until = Some(at.saturating_add(1));
                }
                continue;
            }
            self.maintenance.orders.retain(|o| o.id != order.id);
            self.finish_order(order, at);
        }
        if at < self.maintenance.next_at {
            return;
        }
        let cfg = self.maintenance_config();
        let mut next = self.maintenance.next_at;
        let mut guard = 0u32;
        while at >= next {
            self.roll_maintenance(next);
            self.dispatch_due_plans(next);
            let stepped = next.saturating_add(cfg.telemetry_secs);
            if stepped <= next {
                break;
            }
            next = stepped;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        self.maintenance.next_at = next;
    }

    fn dispatch_due_plans(&mut self, at: u64) {
        let cfg = self.maintenance_config();
        let due: Vec<u64> = self
            .maintenance
            .plans
            .iter()
            .filter(|plan| plan.enabled)
            .filter_map(|plan| {
                let asset = self
                    .maintenance
                    .assets
                    .iter()
                    .find(|a| a.id == plan.asset_id)?;
                if self
                    .maintenance
                    .orders
                    .iter()
                    .any(|o| o.asset_id == plan.asset_id)
                {
                    return None;
                }
                let ready = match plan.kind {
                    PlanKind::Time => {
                        at.saturating_sub(asset.last_service_at) as f64 >= plan.interval
                    }
                    PlanKind::Usage => asset.hours_use - asset.last_service_hours >= plan.interval,
                };
                ready.then_some(plan.asset_id)
            })
            .collect();
        for asset_id in due {
            let _ = self.open_work_order(asset_id, WorkKind::Scheduled, at, &cfg);
        }
    }

    fn open_work_order(
        &mut self,
        asset_id: u64,
        kind: WorkKind,
        at: u64,
        cfg: &MaintenanceConfig,
    ) -> Result<u64, MaintenanceError> {
        let idx = self
            .maintenance
            .assets
            .iter()
            .position(|a| a.id == asset_id)
            .ok_or(MaintenanceError::UnknownAsset)?;
        if self
            .maintenance
            .orders
            .iter()
            .any(|o| o.asset_id == asset_id)
        {
            return Err(MaintenanceError::Busy);
        }
        let asset = &self.maintenance.assets[idx];
        if self.dismantling(asset.coord).is_some_and(|building| {
            asset.key == format!("building:{}:{}", asset.coord, building.as_key())
        }) {
            return Err(MaintenanceError::Busy);
        }
        let cost = Resources::from_parts(cfg.iron_cost, cfg.silicon_cost, 0.0);
        if !self.resources.can_afford(cost) {
            return Err(MaintenanceError::Unaffordable);
        }
        let asset = &self.maintenance.assets[idx];
        if asset.key.ends_with(":fission_reactor") && self.reactor_output_at(asset.coord) > 0.0 {
            self.note_reactor_shutdown(asset.coord, at);
        }
        let asset = &self.maintenance.assets[idx];
        if asset.key.ends_with(":fusion_plant") {
            self.note_fusion_shutdown(asset.coord, at);
        }
        self.resources = self.resources.saturating_sub(cost);
        let ends = match kind {
            WorkKind::Scheduled => at.saturating_add(cfg.work_secs),
            WorkKind::Repair => at.saturating_add(cfg.offline_secs),
        };
        let id = self.maintenance.next_order_id;
        self.maintenance.next_order_id = self.maintenance.next_order_id.saturating_add(1);
        self.maintenance.orders.push(WorkOrder {
            id,
            asset_id,
            kind,
            started_at: at,
            ends_at: ends,
            iron: cfg.iron_cost,
            silicon: cfg.silicon_cost,
        });
        self.maintenance.assets[idx].offline_until = Some(ends);
        Ok(id)
    }

    fn can_pay_work_order(&self, order: &WorkOrder) -> bool {
        if order.kind != WorkKind::Repair {
            return true;
        }
        self.resources
            .can_afford(Resources::from_parts(order.iron, order.silicon, 0.0))
    }

    fn finish_order(&mut self, order: WorkOrder, at: u64) {
        if order.kind == WorkKind::Repair {
            let cost = Resources::from_parts(order.iron, order.silicon, 0.0);
            self.resources = self.resources.saturating_sub(cost);
        }
        let key = {
            let Some(asset) = self
                .maintenance
                .assets
                .iter_mut()
                .find(|a| a.id == order.asset_id)
            else {
                return;
            };
            asset.condition = 1.0;
            asset.last_service_at = at;
            asset.last_service_hours = asset.hours_use;
            asset.offline_until = None;
            asset.key.clone()
        };
        let id = order.id;
        self.record_history(crate::HistoryDraft {
            at,
            kind: HistoryKind::Other,
            actor: "system".into(),
            action: "complete_work_order".into(),
            objects: vec![format!("order:{id}"), key],
            outcome: "serviced".into(),
            ..crate::HistoryDraft::default()
        });
    }

    pub(crate) fn sync_maintenance_assets(&mut self, at: u64) {
        let cfg = self.maintenance_config();
        let mut wanted: Vec<(AssetKind, String, Coord)> = Vec::new();
        let mut sites: Vec<(crate::Buildings, crate::DefenceGrid, Coord)> =
            vec![(self.buildings, self.defences, self.home)];
        for colony in &self.colonies {
            sites.push((colony.buildings, colony.defences, colony.coord));
        }
        for (buildings, defences, coord) in sites {
            for kind in BuildingKind::ALL {
                if buildings.get(kind) > 0 {
                    wanted.push((
                        AssetKind::Building,
                        format!("building:{coord}:{}", kind.as_key()),
                        coord,
                    ));
                }
            }
            for kind in DefenceKind::ALL {
                if defences.get(kind) > 0 {
                    wanted.push((
                        AssetKind::Defence,
                        format!("defence:{coord}:{}", kind.as_key()),
                        coord,
                    ));
                }
            }
        }
        for vehicle in &self.vehicles {
            wanted.push((
                AssetKind::Vehicle,
                format!("vehicle:{}", vehicle.id),
                vehicle.coord,
            ));
        }
        let wanted_keys: HashSet<String> = wanted.iter().map(|(_, key, _)| key.clone()).collect();
        let busy: HashSet<u64> = self
            .maintenance
            .orders
            .iter()
            .map(|order| order.asset_id)
            .collect();
        self.maintenance
            .assets
            .retain(|asset| wanted_keys.contains(&asset.key) || busy.contains(&asset.id));
        let live: HashSet<u64> = self.maintenance.assets.iter().map(|a| a.id).collect();
        self.maintenance
            .plans
            .retain(|plan| live.contains(&plan.asset_id));
        for (kind, key, coord) in wanted {
            if let Some(asset) = self.maintenance.assets.iter_mut().find(|a| a.key == key) {
                asset.coord = coord;
                continue;
            }
            let id = self.maintenance.next_id;
            self.maintenance.next_id = self.maintenance.next_id.saturating_add(1);
            self.maintenance.assets.push(Asset {
                id,
                kind,
                key,
                coord,
                condition: 1.0,
                hours_use: 0.0,
                installed_at: at,
                last_service_at: at,
                last_service_hours: 0.0,
                offline_until: None,
            });
            let plan_id = self.maintenance.next_plan_id;
            self.maintenance.next_plan_id = self.maintenance.next_plan_id.saturating_add(1);
            self.maintenance.plans.push(MaintenancePlan {
                id: plan_id,
                asset_id: id,
                kind: PlanKind::Time,
                interval: cfg.interval_secs as f64,
                enabled: true,
            });
        }
    }

    fn roll_maintenance(&mut self, at: u64) {
        let cfg = self.maintenance_config();
        let hours = cfg.telemetry_secs as f64 / 3600.0;
        let seed = self.rng_seed;
        let ids: Vec<u64> = self.maintenance.assets.iter().map(|a| a.id).collect();
        for id in ids {
            self.sample_asset(id, at, hours, seed);
        }
    }

    fn sample_asset(&mut self, id: u64, at: u64, hours: f64, seed: u64) {
        let cfg = self.maintenance_config();
        let Some(idx) = self.maintenance.assets.iter().position(|a| a.id == id) else {
            return;
        };
        let asset = &self.maintenance.assets[idx];
        if self.dismantling(asset.coord).is_some_and(|building| {
            asset.key == format!("building:{}:{}", asset.coord, building.as_key())
        }) {
            return;
        }
        let coord = self.maintenance.assets[idx].coord;
        let heat = self.waste_at(coord);
        let overflow = heat.as_ref().is_some_and(|h| h.overflowing());
        let heat_ratio = heat.as_ref().map(|h| h.heat_ratio()).unwrap_or(0.0);
        let offline = self.maintenance.assets[idx]
            .offline_until
            .is_some_and(|until| until > at);
        let just_serviced = self.maintenance.assets[idx].last_service_at == at;
        if !offline && !just_serviced {
            let use_w = cfg.wear_use_per_hour * hours;
            let age_w = cfg.wear_age_per_hour * hours;
            let reactor_at_cap = self.maintenance.assets[idx]
                .key
                .ends_with(":fission_reactor")
                && self.reactor_output_at(coord) > 0.0
                && heat_ratio >= self.config.waste.warn_ratio;
            let heat_w = if overflow || reactor_at_cap {
                cfg.wear_heat_per_hour * hours
            } else {
                0.0
            };
            let neutron_w = if self.maintenance.assets[idx].key.ends_with(":fusion_plant") {
                self.fusion_neutron_wear_per_hour(coord) * hours
            } else {
                0.0
            };
            let cond =
                (self.maintenance.assets[idx].condition - use_w - age_w - heat_w - neutron_w)
                    .clamp(0.05, 1.0);
            self.maintenance.assets[idx].condition = cond;
            self.maintenance.assets[idx].hours_use += hours;
        }
        let asset = &self.maintenance.assets[idx];
        let n = unit_noise(seed, id, at) * cfg.noise;
        let temp = (heat_ratio + n).clamp(0.0, 2.0);
        let vib = ((1.0 - asset.condition) + n.abs()).clamp(0.0, 2.0);
        let power = if offline {
            0.0
        } else {
            (1.0 + n).clamp(0.0, 2.0)
        };
        let reading_id = self.maintenance.next_reading_id;
        self.maintenance.next_reading_id = self.maintenance.next_reading_id.saturating_add(1);
        let reading = TelemetryReading {
            id: reading_id,
            asset_id: id,
            at,
            temperature: temp,
            vibration: vib,
            power,
            hours_use: asset.hours_use,
        };
        self.maintenance.readings.push(reading.clone());
        let cap = cfg.archive_cap.max(1);
        if self.maintenance.readings.len() > cap {
            let drop = self.maintenance.readings.len() - cap;
            self.maintenance.readings.drain(0..drop);
        }
        let p_hour = fail_p_per_hour(&cfg, asset.hours_use, asset.condition, overflow)
            * (1.0 + 2.0 * (vib - 0.5).max(0.0));
        let probability = predict_failure(&cfg, p_hour.clamp(0.0, 0.5));
        let pred_id = self.maintenance.next_prediction_id;
        self.maintenance.next_prediction_id = self.maintenance.next_prediction_id.saturating_add(1);
        self.maintenance.predictions.push(FailurePrediction {
            id: pred_id,
            asset_id: id,
            at,
            probability,
            horizon_secs: cfg.horizon_secs,
            reading_id,
        });
        let cap = cfg.archive_cap.max(1);
        if self.maintenance.predictions.len() > cap {
            let drop = self.maintenance.predictions.len() - cap;
            self.maintenance.predictions.drain(0..drop);
        }
        if offline || just_serviced {
            return;
        }
        let roll =
            (splitmix(seed ^ id.wrapping_mul(0xA24B_AED4_96E9_C165) ^ at) as f64) / u64::MAX as f64;
        if roll < p_hour.clamp(0.0, 0.5) * hours {
            self.fail_asset(id, at, &cfg);
        }
    }

    fn fail_asset(&mut self, id: u64, at: u64, cfg: &MaintenanceConfig) {
        let Some(idx) = self.maintenance.assets.iter().position(|a| a.id == id) else {
            return;
        };
        if self.maintenance.orders.iter().any(|o| o.asset_id == id) {
            return;
        }
        let key = self.maintenance.assets[idx].key.clone();
        if key.ends_with(":fission_reactor")
            && self.reactor_output_at(self.maintenance.assets[idx].coord) > 0.0
        {
            self.note_reactor_shutdown(self.maintenance.assets[idx].coord, at);
        }
        if key.ends_with(":fusion_plant") {
            self.note_fusion_shutdown(self.maintenance.assets[idx].coord, at);
        }
        let cond = (self.maintenance.assets[idx].condition * 0.5).clamp(0.05, 1.0);
        self.maintenance.assets[idx].condition = cond;
        let ends = at.saturating_add(cfg.offline_secs);
        self.maintenance.assets[idx].offline_until = Some(ends);
        let order_id = self.maintenance.next_order_id;
        self.maintenance.next_order_id = self.maintenance.next_order_id.saturating_add(1);
        self.maintenance.orders.push(WorkOrder {
            id: order_id,
            asset_id: id,
            kind: WorkKind::Repair,
            started_at: at,
            ends_at: ends,
            iron: cfg.iron_cost,
            silicon: cfg.silicon_cost,
        });
        self.record_history(crate::HistoryDraft {
            at,
            kind: HistoryKind::Other,
            actor: "system".into(),
            action: "asset_failure".into(),
            objects: vec![format!("asset:{id}"), key.clone()],
            outcome: "offline".into(),
            ..crate::HistoryDraft::default()
        });
        self.push_event(at, format!("asset {key} failed"));
        self.note_failure_incident(&key, at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::incident::IncidentKind;
    use crate::leaders::Actor;
    use crate::state::GameState;

    #[test]
    fn reactor_failure_shuts_down_without_destroying_the_building() {
        let mut state = GameState::new(1_000);
        state.buildings.fission_reactor = 1;
        state.buildings.radiator = 3;
        state.goods.insert("reactor_fuel".into(), 10.0);
        state.sync_maintenance_assets(1_000);
        let asset = state
            .maintenance
            .assets
            .iter()
            .find(|asset| asset.key.ends_with(":fission_reactor"))
            .unwrap()
            .id;
        assert_eq!(state.reactor_output_at(state.home), 60.0);
        state
            .maintenance
            .assets
            .iter_mut()
            .find(|a| a.id == asset)
            .unwrap()
            .condition = 0.4;
        assert!((state.reactor_output_at(state.home) - 42.0).abs() < 1e-9);
        let config = state.maintenance_config();
        state.fail_asset(asset, 1_000, &config);
        assert_eq!(state.reactor_output_at(state.home), 0.0);
        assert_eq!(state.buildings.fission_reactor, 1);
        assert_eq!(
            state
                .maintenance
                .assets
                .iter()
                .find(|a| a.id == asset)
                .unwrap()
                .offline_until,
            Some(1_000 + config.offline_secs)
        );
        assert!(state.reactor_decay_heat_at(state.home) > 0.0);
        assert!(
            state
                .incidents
                .incidents
                .iter()
                .any(|i| i.kind == IncidentKind::Failure)
        );
        state.last_tick_unix += config.offline_secs;
        state
            .maintenance
            .orders
            .retain(|order| order.asset_id != asset);
        assert!(state.reactor_decay_heat_at(state.home) > 0.0);
        assert!(state.reactor_output_at(state.home) > 0.0);
    }

    #[test]
    fn fusion_disruption_needs_a_new_startup_and_keeps_the_building() {
        let mut state = GameState::new(1_000);
        state.buildings.fusion_plant = 1;
        state.buildings.radiator = 20;
        state.resources.deuterium = 100.0;
        state.fusion_plants.insert(
            state.home,
            crate::FusionPlant {
                mode: crate::FusionMode::Dd,
                startup_left: 0.0,
                activated: true,
                stopped_at: None,
                residual_heat: 0.0,
            },
        );
        state.sync_maintenance_assets(1_000);
        let asset = state
            .maintenance
            .assets
            .iter()
            .find(|asset| asset.key.ends_with(":fusion_plant"))
            .unwrap()
            .id;
        assert!(state.fusion_output_at(state.home) > 0.0);
        let config = state.maintenance_config();
        state.fail_asset(asset, 1_000, &config);
        assert_eq!(state.fusion_output_at(state.home), 0.0);
        assert_eq!(state.buildings.fusion_plant, 1);
        let residual = state
            .config
            .fusion_plant
            .residual_heat(crate::FusionMode::Dd, 1);
        assert_eq!(state.fusion_heat_at(state.home), residual);
        assert!(
            state
                .incidents
                .incidents
                .iter()
                .any(|i| i.kind == IncidentKind::Failure)
        );
        state.last_tick_unix += config.offline_secs;
        state
            .maintenance
            .orders
            .retain(|order| order.asset_id != asset);
        assert_eq!(state.fusion_output_at(state.home), 0.0);
        assert!(!state.fusion_plants[&state.home].burning());
    }

    #[test]
    fn scheduled_reactor_service_records_decay_heat() {
        let mut state = GameState::new(1_000);
        state.buildings.fission_reactor = 1;
        state.buildings.radiator = 3;
        state.goods.insert("reactor_fuel".into(), 10.0);
        state.sync_maintenance_assets(1_000);
        let id = state
            .maintenance
            .assets
            .iter()
            .find(|asset| asset.key.ends_with(":fission_reactor"))
            .unwrap()
            .id;
        state
            .schedule_maintenance(id, crate::PlanKind::Time, Actor::Player, 1_000)
            .unwrap();
        assert_eq!(state.reactor_output_at(state.home), 0.0);
        assert!(state.reactor_decay_heat_at(state.home) > 0.0);
        assert_eq!(state.reactor_shutdowns[&state.home].at, 1_000);
    }

    #[test]
    fn bundled_maintenance_parse() {
        let cfg = MaintenanceConfig::bundled();
        assert!(cfg.telemetry_secs > 0);
        assert!(cfg.noise <= 0.5);
    }

    #[test]
    fn prediction_formula_matches_fixed_inputs() {
        let cfg = MaintenanceConfig::bundled();
        let p = fail_p_per_hour(&cfg, 10.0, 0.2, false);
        let pred = predict_failure(&cfg, p);
        let hours = cfg.horizon_secs as f64 / 3600.0;
        let expect = 1.0 - (1.0 - p).powf(hours);
        assert!((pred - expect).abs() < 1e-12);
        assert!(pred > 0.0);
        let healthy = fail_p_per_hour(&cfg, 200.0, 1.0, false);
        assert!(healthy < p);
    }

    #[test]
    fn heat_and_use_wear_the_mines() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.sync_maintenance_assets(1_000);
        let before = state.maintenance.assets[0].condition;
        state.buildings.metal_mine = 40;
        state.buildings.crystal_mine = 40;
        state.buildings.solar_plant = 1;
        let due = state.maintenance.next_at;
        state.catch_up(due);
        let after = state
            .maintenance
            .assets
            .iter()
            .find(|a| a.key.contains("metal_mine"))
            .map(|a| a.condition)
            .unwrap_or(1.0);
        assert!(after < before);
        assert!(!state.maintenance.readings.is_empty());
        assert!(!state.maintenance.predictions.is_empty());
    }

    #[test]
    fn schedule_and_complete_restore_condition() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 1_000.0;
        state.resources.silicon = 1_000.0;
        state.sync_maintenance_assets(1_000);
        let id = state.maintenance.assets[0].id;
        state.maintenance.assets[0].condition = 0.4;
        let order = state
            .schedule_maintenance(id, PlanKind::Time, Actor::Player, 1_000)
            .unwrap();
        assert_eq!(
            state.building_health(state.home, BuildingKind::MetalMine),
            0.0
        );
        let ends = state.maintenance.orders[0].ends_at;
        state.catch_up(ends);
        assert!((state.maintenance.assets[0].condition - 1.0).abs() < 1e-9);
        assert!(state.maintenance.orders.iter().all(|o| o.id != order));
        state.maintenance.assets[0].condition = 0.4;
        let too_soon = state
            .schedule_maintenance(id, PlanKind::Time, Actor::Player, ends)
            .unwrap();
        assert_eq!(
            state.complete_work_order(too_soon, Actor::Player, ends),
            Err(MaintenanceError::NotDue)
        );
        let due_ends = state.maintenance.orders[0].ends_at;
        state
            .complete_work_order(too_soon, Actor::Player, due_ends)
            .unwrap();
        assert!(
            state
                .complete_work_order(too_soon, Actor::Player, due_ends)
                .is_ok()
        );
    }

    #[test]
    fn time_plan_opens_a_work_order_when_the_interval_elapses() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 1_000.0;
        state.resources.silicon = 1_000.0;
        state.sync_maintenance_assets(1_000);
        let id = state.maintenance.assets[0].id;
        state.maintenance.plans[0].interval = 3600.0;
        state.maintenance.assets[0].last_service_at = 1_000;
        state.catch_up(state.maintenance.next_at);
        assert!(
            state
                .maintenance
                .orders
                .iter()
                .any(|o| o.asset_id == id && o.kind == WorkKind::Scheduled),
            "{:?}",
            state.maintenance.orders
        );
    }

    #[test]
    fn failure_opens_an_incident_and_is_deterministic() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut a = GameState::with_seed(1_000, cfg.clone(), 9);
        let mut b = GameState::with_seed(1_000, cfg, 9);
        a.sync_maintenance_assets(1_000);
        b.sync_maintenance_assets(1_000);
        let id = a.maintenance.assets[0].id;
        let maint = a.maintenance_config();
        a.fail_asset(id, 1_000, &maint);
        b.fail_asset(id, 1_000, &maint);
        assert_eq!(
            a.maintenance.assets[0].offline_until,
            b.maintenance.assets[0].offline_until
        );
        assert!(
            a.incident_snapshot()
                .open
                .iter()
                .any(|i| i.kind == IncidentKind::Failure)
        );
        assert_eq!(
            a.incident_snapshot().open.len(),
            b.incident_snapshot().open.len()
        );
    }

    #[test]
    fn colony_heat_wears_local_assets_not_home() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.colonies.push(crate::Colony {
            coord,
            kind: crate::PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: crate::Buildings {
                metal_mine: 40,
                crystal_mine: 40,
                solar_plant: 1,
                ..crate::Buildings::default()
            },
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.sync_maintenance_assets(1_000);
        let due = state.maintenance.next_at;
        state.catch_up(due);
        let home_cond = state
            .maintenance
            .assets
            .iter()
            .find(|a| a.coord == state.home && a.key.contains("metal_mine"))
            .map(|a| a.condition)
            .unwrap();
        let colony_cond = state
            .maintenance
            .assets
            .iter()
            .find(|a| a.coord == coord && a.key.contains("metal_mine"))
            .map(|a| a.condition)
            .unwrap();
        assert!(
            colony_cond < home_cond,
            "colony {colony_cond} home {home_cond}"
        );
        assert!(state.waste_at(coord).is_some_and(|w| w.overflowing()));
        assert!(!state.waste_home().overflowing());
    }

    #[test]
    fn colony_offline_mine_stops_non_chain_output() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.chains.recipes.clear();
        let mut state = GameState::with_config(1_000, cfg);
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.colonies.push(crate::Colony {
            coord,
            kind: crate::PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: crate::Buildings {
                metal_mine: 8,
                solar_plant: 8,
                ..crate::Buildings::default()
            },
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.sync_maintenance_assets(1_000);
        let id = state
            .maintenance
            .assets
            .iter()
            .find(|a| a.coord == coord && a.key.contains("metal_mine"))
            .unwrap()
            .id;
        let maint = state.maintenance_config();
        state.fail_asset(id, 1_000, &maint);
        assert_eq!(state.building_health(coord, BuildingKind::MetalMine), 0.0);
        state.catch_up(1_000 + 3_600);
        assert!(
            state.colonies[0].resources.iron.abs() < 1e-9,
            "offline colony still produced {}",
            state.colonies[0].resources.iron
        );
    }

    #[test]
    fn colony_mines_are_tracked_as_assets() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.colonies.push(crate::Colony {
            coord,
            kind: crate::PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: crate::Buildings {
                metal_mine: 4,
                ..crate::Buildings::default()
            },
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.sync_maintenance_assets(1_000);
        assert!(
            state
                .maintenance
                .assets
                .iter()
                .any(|a| a.key.contains("metal_mine") && a.coord == coord),
            "{:?}",
            state
                .maintenance
                .assets
                .iter()
                .map(|a| &a.key)
                .collect::<Vec<_>>()
        );
        assert!(state.building_health(coord, BuildingKind::MetalMine) > 0.0);
    }

    #[test]
    fn catch_up_matches_hourly_ticks_across_telemetry() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut stepped = GameState::with_config(1_000, cfg.clone());
        let mut gap = GameState::with_config(1_000, cfg);
        stepped.sync_maintenance_assets(1_000);
        gap.sync_maintenance_assets(1_000);
        let later = 1_000 + 3 * stepped.maintenance_config().telemetry_secs;
        let mut at = 1_000;
        while at < later {
            at += 1_800;
            stepped.catch_up(at);
        }
        gap.catch_up(later);
        assert_eq!(stepped.last_tick_unix, gap.last_tick_unix);
        assert!((stepped.resources.iron - gap.resources.iron).abs() < 1e-6);
        let stepped_cond = stepped
            .maintenance
            .assets
            .iter()
            .find(|a| a.key.contains("metal_mine"))
            .map(|a| a.condition)
            .unwrap();
        let gap_cond = gap
            .maintenance
            .assets
            .iter()
            .find(|a| a.key.contains("metal_mine"))
            .map(|a| a.condition)
            .unwrap();
        assert!(
            (stepped_cond - gap_cond).abs() < 1e-9,
            "{stepped_cond} vs {gap_cond}"
        );
        assert_eq!(
            stepped.maintenance.readings.len(),
            gap.maintenance.readings.len()
        );
    }

    #[test]
    fn unpaid_repair_keeps_the_asset_offline() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.sync_maintenance_assets(1_000);
        let id = state
            .maintenance
            .assets
            .iter()
            .find(|a| a.key.contains("metal_mine"))
            .unwrap()
            .id;
        let kind = BuildingKind::MetalMine;
        let maint = state.maintenance_config();
        state.fail_asset(id, 1_000, &maint);
        state.resources.iron = 0.0;
        state.resources.silicon = 0.0;
        let ends = state.maintenance.orders[0].ends_at;
        state.catch_up(ends.saturating_add(3_600));
        assert_eq!(state.building_health(state.home, kind), 0.0);
        assert_eq!(state.maintenance.orders.len(), 1);
        assert!(
            state
                .maintenance
                .assets
                .iter()
                .find(|a| a.id == id)
                .unwrap()
                .offline_until
                .is_some_and(|until| until > state.last_tick_unix)
        );
        state.resources.iron = 1_000.0;
        state.resources.silicon = 1_000.0;
        state.catch_up(state.last_tick_unix.saturating_add(1));
        assert!(state.maintenance.orders.is_empty());
        assert!(state.building_health(state.home, kind) > 0.0);
    }

    #[test]
    fn repair_orders_charge_parts_when_completed() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 1_000.0;
        state.resources.silicon = 1_000.0;
        state.sync_maintenance_assets(1_000);
        let id = state.maintenance.assets[0].id;
        let maint = state.maintenance_config();
        state.fail_asset(id, 1_000, &maint);
        let iron_before = state.resources.iron;
        let order = state.maintenance.orders[0].id;
        let ends = state.maintenance.orders[0].ends_at;
        state.catch_up(ends);
        assert!(state.resources.iron < iron_before);
        assert!(state.maintenance.orders.iter().all(|o| o.id != order));
    }

    #[test]
    fn moving_vehicles_update_asset_coordinates() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        state.vehicles.push(crate::Vehicle {
            id: 7,
            name: "barge".into(),
            ships: crate::Fleet::default(),
            coord: state.home,
            free_at: 1_000,
        });
        state.sync_maintenance_assets(1_000);
        assert_eq!(
            state
                .maintenance
                .assets
                .iter()
                .find(|a| a.key == "vehicle:7")
                .map(|a| a.coord),
            Some(state.home)
        );
        state.vehicles[0].coord = dest;
        state.sync_maintenance_assets(2_000);
        assert_eq!(
            state
                .maintenance
                .assets
                .iter()
                .find(|a| a.key == "vehicle:7")
                .map(|a| a.coord),
            Some(dest)
        );
    }

    #[test]
    fn dropped_vehicles_leave_the_asset_list() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.vehicles.push(crate::Vehicle {
            id: 7,
            name: "barge".into(),
            ships: crate::Fleet::default(),
            coord: state.home,
            free_at: 1_000,
        });
        state.sync_maintenance_assets(1_000);
        assert!(
            state
                .maintenance
                .assets
                .iter()
                .any(|a| a.key == "vehicle:7")
        );
        state.vehicles.clear();
        state.sync_maintenance_assets(1_000);
        assert!(
            state
                .maintenance
                .assets
                .iter()
                .all(|a| a.key != "vehicle:7")
        );
    }
}
