//! Durable shard host: `ShardSession` maths, tenant saves, signed view posts.

use crate::webhook::{
    AdmitOutcome, DeliveryPlane, WEBHOOK_DELIVERY_CONTRACT, WebhookDelivery, sign_delivery,
    signing_key,
};
use hoshi_core::{
    Actor, BalanceConfig, BuildingKind, Clock, Coord, Fleet, GameState, Intent, ShardBookkeeping,
    ShardCommand, ShardCommons, ShardConfig, ShardError, ShardMap, ShardRaid, ShardSession,
    ShardView, ShipKind, load_tenants,
};
use hoshi_storage::{load_state, open_save, save_state};
use sekai_source_sdk::{
    Connector, ConnectorFailure, RecordChange, SourceAdapterConfig, SourceRecord,
    build_registered_source_batch,
};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Registered object-sync source. The plane maps signed deliveries onto
/// `ApplySourceBatch`; GitHub-only webhook admit is a different receiver.
pub const SOURCE: &str = "hoshi";
pub const TYPE_DIGEST: &str =
    "sha256:b4005f9b4019c320cc4078edfc2bc4fd06a630b861b50db96fae706c826f9d19";
const RECORD_KIND: &str = "TenantView";
const KEY_ID: &str = "shard";

/// Host clock the connector moves; `ShardSession` holds a clone.
#[derive(Clone)]
pub struct HostClock(Rc<Cell<u64>>);

impl HostClock {
    pub fn new(now: u64) -> Self {
        Self(Rc::new(Cell::new(now)))
    }

    pub fn set(&self, now: u64) {
        self.0.set(now);
    }
}

impl Clock for HostClock {
    fn now_unix(&self) -> u64 {
        self.0.get()
    }
}

pub struct HostParams {
    pub persist_dir: PathBuf,
    pub now: u64,
    pub map: ShardMap,
    pub tenant_cap: u32,
    pub newbie_secs: u64,
    pub namespace: String,
    pub source_instance: String,
    pub producer_identity: String,
    /// Credential: webhook signing seed. Never written to a view or save.
    pub signing_seed: [u8; 32],
    /// Credential: bridge MAC. Never written to a view or save.
    pub bridge_mac: [u8; 32],
}

/// One host process: clock, session, tenant copies, and the plane it posts to.
pub struct ShardHost<P> {
    clock: HostClock,
    shard: ShardSession<HostClock>,
    empires: BTreeMap<String, GameState>,
    map: ShardMap,
    persist_dir: PathBuf,
    /// Exclusive fence for this persist dir; released on drop.
    #[allow(dead_code)]
    lock: fs::File,
    rules: BalanceConfig,
    namespace: String,
    source_instance: String,
    producer_identity: String,
    signing_seed: [u8; 32],
    plane: P,
    revisions: BTreeMap<String, u64>,
    /// Last posted payload digest per identity; a revision bumps only when this changes.
    digests: BTreeMap<String, String>,
}

impl<P: DeliveryPlane> ShardHost<P> {
    pub fn create(params: HostParams, plane: P) -> Result<Self, ConnectorFailure> {
        let lock = acquire_lock(&params.persist_dir)?;
        let rules = rules();
        let clock = HostClock::new(params.now);
        let config = ShardConfig {
            newbie_secs: params.newbie_secs,
            tenant_cap: params.tenant_cap,
            market: rules.market.clone(),
            ..ShardConfig::default()
        };
        Ok(Self {
            shard: ShardSession::new(clock.clone(), params.bridge_mac, config),
            clock,
            empires: BTreeMap::new(),
            map: params.map,
            persist_dir: params.persist_dir,
            lock,
            rules,
            namespace: params.namespace,
            source_instance: params.source_instance,
            producer_identity: params.producer_identity,
            signing_seed: params.signing_seed,
            plane,
            revisions: BTreeMap::new(),
            digests: BTreeMap::new(),
        })
    }

    /// Rebuild from the live generation (`current`, else `prev`). Missing dir is empty.
    pub fn restore(params: HostParams, plane: P) -> Result<Self, ConnectorFailure> {
        let mut host = Self::create(params, plane)?;
        host.load_generation()?;
        Ok(host)
    }

    pub fn set_now(&self, now: u64) {
        self.clock.set(now);
    }

    pub fn now(&self) -> u64 {
        self.clock.now_unix()
    }

    pub fn plane(&self) -> &P {
        &self.plane
    }

    pub fn plane_mut(&mut self) -> &mut P {
        &mut self.plane
    }

    #[cfg(test)]
    pub(crate) fn save_now(&mut self) {
        self.commit_generation(None).unwrap();
    }

    fn commit_generation(
        &mut self,
        extra: Option<(&str, &WebhookDelivery, &str)>,
    ) -> Result<(), ConnectorFailure> {
        let staging = self.persist_dir.join("staging");
        let current = self.persist_dir.join("current");
        let prev = self.persist_dir.join("prev");
        let live = live_generation(&self.persist_dir);
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|_| ConnectorFailure::Unreachable)?;
        }
        fs::create_dir_all(staging.join("tenants")).map_err(|_| ConnectorFailure::Unreachable)?;
        fs::create_dir_all(staging.join("deliveries"))
            .map_err(|_| ConnectorFailure::Unreachable)?;
        // Keep every delivery so a duplicate idempotency key can replay. Pruning
        // acked files is a later store concern, not this host's generation swap.
        copy_tree_files(&live.join("deliveries"), &staging.join("deliveries"))?;
        if let Some((id, delivery, change_digest)) = extra {
            write_atomic(
                &delivery_path(&staging, id),
                &serde_json::to_vec(delivery).map_err(|_| ConnectorFailure::Invalid)?,
            )?;
            let mut changes = read_digests(&live.join("changes"))?;
            changes.insert(id.to_string(), change_digest.to_string());
            let encoded = changes
                .iter()
                .map(|(key, digest)| format!("{key}\t{digest}"))
                .collect::<Vec<_>>()
                .join("\n");
            write_atomic(&staging.join("changes"), encoded.as_bytes())?;
        } else {
            copy_if_exists(&live.join("changes"), &staging.join("changes"))?;
        }
        copy_if_exists(&live.join("cursor"), &staging.join("cursor"))?;
        copy_if_exists(&live.join("acked"), &staging.join("acked"))?;
        copy_if_exists(&live.join("pending"), &staging.join("pending"))?;
        if let Some((id, _, _)) = extra {
            let mut pending = read_lines(&staging.join("pending"))?;
            if !pending.iter().any(|queued| queued == id) {
                pending.push(id.to_string());
                write_atomic(&staging.join("pending"), pending.join("\n").as_bytes())?;
            }
        }
        for (identity, state) in &self.empires {
            let mut conn = open_save(&tenant_save(&staging, identity))
                .map_err(|_| ConnectorFailure::Unreachable)?;
            save_state(&mut conn, state).map_err(|_| ConnectorFailure::Unreachable)?;
        }
        let bookkeeping = self
            .shard
            .export_bookkeeping()
            .map_err(|_| ConnectorFailure::Invalid)?;
        write_atomic(&staging.join("bookkeeping"), bookkeeping.as_bytes())?;
        let commons = self
            .shard
            .commons()
            .to_bytes()
            .map_err(|_| ConnectorFailure::Invalid)?;
        write_atomic(&staging.join("commons"), &commons)?;
        let identities = self.empires.keys().cloned().collect::<Vec<_>>().join("\n");
        write_atomic(&staging.join("identities"), identities.as_bytes())?;
        let revisions = self
            .revisions
            .iter()
            .map(|(identity, revision)| format!("{identity}\t{revision}"))
            .collect::<Vec<_>>()
            .join("\n");
        write_atomic(&staging.join("revisions"), revisions.as_bytes())?;
        let digests = self
            .digests
            .iter()
            .map(|(identity, digest)| format!("{identity}\t{digest}"))
            .collect::<Vec<_>>()
            .join("\n");
        write_atomic(&staging.join("digests"), digests.as_bytes())?;
        sync_tree(&staging)?;
        if current.join("identities").exists() {
            if prev.exists() {
                fs::remove_dir_all(&prev).map_err(|_| ConnectorFailure::Unreachable)?;
            }
            fs::rename(&current, &prev).map_err(|_| ConnectorFailure::Unreachable)?;
        } else if current.exists() {
            fs::remove_dir_all(&current).map_err(|_| ConnectorFailure::Unreachable)?;
        }
        fs::rename(&staging, &current).map_err(|_| ConnectorFailure::Unreachable)?;
        sync_dir(&self.persist_dir)
    }

    fn build_delivery(
        &mut self,
        delivery_id: &str,
        actor: Option<&str>,
    ) -> Result<WebhookDelivery, ConnectorFailure> {
        let mut records = Vec::new();
        let identities: Vec<String> = self.empires.keys().cloned().collect();
        let observed_at_ms = i64::try_from(self.now().saturating_mul(1000)).unwrap_or(i64::MAX);
        for identity in identities {
            let view = self
                .shard
                .view(&identity, &mut self.empires)
                .map_err(map_shard)?;
            let properties = view_properties(&view);
            let digest = version_digest(&properties);
            let actor_changed = actor == Some(identity.as_str());
            let view_changed = self.digests.get(&identity) != Some(&digest);
            if actor_changed || view_changed {
                self.bump(&identity);
                self.digests.insert(identity.clone(), digest);
            }
            let revision = self.revisions.get(&identity).copied().unwrap_or(0);
            records.push(view_record(
                &self.source_instance,
                view.identity.clone(),
                properties,
                observed_at_ms,
                revision,
            ));
        }
        let cursor_now = format!("at-{}:{delivery_id}", self.now());
        let previous = previous_cursor(&self.persist_dir);
        let config = SourceAdapterConfig {
            namespace: self.namespace.clone(),
            producer_identity: self.producer_identity.clone(),
            source_instance: self.source_instance.clone(),
            type_digest: TYPE_DIGEST.into(),
        };
        let batch = build_registered_source_batch(
            &config,
            SOURCE,
            previous.as_deref().unwrap_or(""),
            &cursor_now,
            observed_at_ms,
            records,
        )
        .map_err(|_| ConnectorFailure::Invalid)?;
        let unsigned = WebhookDelivery {
            contract_version: WEBHOOK_DELIVERY_CONTRACT.into(),
            delivery_id: delivery_id.into(),
            namespace: batch.namespace,
            producer_identity: batch.producer_identity,
            source: batch.source,
            source_instance: batch.source_instance,
            type_digest: batch.type_digest,
            current_cursor: batch.current_cursor,
            proposed_next_cursor: batch.proposed_next_cursor,
            signed_at_ms: observed_at_ms,
            not_after_ms: observed_at_ms.saturating_add(60_000),
            records: batch.records,
            content_digest: String::new(),
            signature: None,
        };
        sign_delivery(unsigned, &signing_key(&self.signing_seed), KEY_ID)
    }

    fn post_views(&mut self, delivery_id: &str) -> Result<AdmitOutcome, ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        if read_lines(&generation.join("acked"))?
            .iter()
            .any(|id| id == delivery_id)
        {
            return Ok(AdmitOutcome::ExactReplay);
        }
        let mut stored = load_delivery(&generation, delivery_id)?;
        if let Some(delivery) = stored.as_ref()
            && delivery_expired(delivery, self.now())
        {
            let resigned = resign_delivery(delivery, self.now(), &self.signing_seed)?;
            write_atomic(
                &delivery_path(&generation, delivery_id),
                &serde_json::to_vec(&resigned).map_err(|_| ConnectorFailure::Invalid)?,
            )?;
            stored = Some(resigned);
        }
        let signed = stored.ok_or(ConnectorFailure::Invalid)?;
        let outcome = self.plane.admit(&signed)?;
        write_atomic(
            &generation.join("cursor"),
            signed.proposed_next_cursor.as_bytes(),
        )?;
        let mut acked = read_lines(&generation.join("acked"))?;
        if !acked.iter().any(|id| id == delivery_id) {
            acked.push(delivery_id.to_string());
            write_atomic(&generation.join("acked"), acked.join("\n").as_bytes())?;
        }
        let pending: Vec<String> = read_lines(&generation.join("pending"))?
            .into_iter()
            .filter(|id| id != delivery_id)
            .collect();
        write_atomic(&generation.join("pending"), pending.join("\n").as_bytes())?;
        Ok(outcome)
    }

    fn publish_views(&mut self, delivery_id: &str) -> Result<(), ConnectorFailure> {
        match self.post_views(delivery_id) {
            Ok(_) => Ok(()),
            Err(ConnectorFailure::Unreachable | ConnectorFailure::AuthFailed) => {
                Err(ConnectorFailure::Unreachable)
            }
            Err(_) => self.ack_without_cursor(delivery_id),
        }
    }

    fn ack_without_cursor(&mut self, delivery_id: &str) -> Result<(), ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        let mut acked = read_lines(&generation.join("acked"))?;
        if !acked.iter().any(|id| id == delivery_id) {
            acked.push(delivery_id.to_string());
            write_atomic(&generation.join("acked"), acked.join("\n").as_bytes())?;
        }
        self.drop_pending(delivery_id)
    }

    fn drain_pending(&mut self) -> Result<(), ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        for id in read_lines(&generation.join("pending"))? {
            self.publish_views(&id)?;
        }
        Ok(())
    }

    fn drop_pending(&mut self, delivery_id: &str) -> Result<(), ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        let pending: Vec<String> = read_lines(&generation.join("pending"))?
            .into_iter()
            .filter(|id| id != delivery_id)
            .collect();
        write_atomic(&generation.join("pending"), pending.join("\n").as_bytes())
    }

    fn checkpoint(&mut self) -> Result<HostCheckpoint, ConnectorFailure> {
        Ok(HostCheckpoint {
            empires: self.empires.clone(),
            bookkeeping: self
                .shard
                .export_bookkeeping()
                .map_err(|_| ConnectorFailure::Invalid)?
                .as_bytes()
                .to_vec(),
            commons: self
                .shard
                .commons()
                .to_bytes()
                .map_err(|_| ConnectorFailure::Invalid)?,
            revisions: self.revisions.clone(),
            digests: self.digests.clone(),
        })
    }

    fn revert(&mut self, checkpoint: HostCheckpoint) -> Result<(), ConnectorFailure> {
        self.shard
            .restore_bookkeeping(&ShardBookkeeping::from_bytes(checkpoint.bookkeeping))
            .map_err(|_| ConnectorFailure::Invalid)?;
        *self.shard.commons_mut() =
            ShardCommons::from_bytes(&checkpoint.commons).map_err(|_| ConnectorFailure::Invalid)?;
        self.empires = checkpoint.empires;
        self.revisions = checkpoint.revisions;
        self.digests = checkpoint.digests;
        Ok(())
    }

    fn load_generation(&mut self) -> Result<(), ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        let identities_path = generation.join("identities");
        if !identities_path.exists() {
            self.empires.clear();
            return Ok(());
        }
        let identities = read_identities(&identities_path)?;
        let bookkeeping =
            fs::read(generation.join("bookkeeping")).map_err(|_| ConnectorFailure::Unreachable)?;
        let commons =
            fs::read(generation.join("commons")).map_err(|_| ConnectorFailure::Unreachable)?;
        self.shard
            .restore_bookkeeping(&ShardBookkeeping::from_bytes(bookkeeping))
            .map_err(|_| ConnectorFailure::Invalid)?;
        *self.shard.commons_mut() =
            ShardCommons::from_bytes(&commons).map_err(|_| ConnectorFailure::Invalid)?;
        let rules = self.rules.clone();
        let (empires, failed) = load_tenants(identities, |identity| {
            let conn = open_save(&tenant_save(&generation, identity))?;
            load_state(&conn, rules.clone())
        });
        if !failed.is_empty() {
            return Err(ConnectorFailure::Invalid);
        }
        self.empires = empires;
        self.revisions = read_revisions(&generation.join("revisions"))?;
        self.digests = read_digests(&generation.join("digests"))?;
        Ok(())
    }

    fn bump(&mut self, identity: &str) {
        let next = self.revisions.get(identity).copied().unwrap_or(0) + 1;
        self.revisions.insert(identity.to_string(), next);
    }

    fn reject_stale_version(&mut self, change: &RecordChange) -> Result<(), ConnectorFailure> {
        if change.expected_version.is_empty() {
            return Ok(());
        }
        let identity = identity_of(change)?;
        match self.revisions.get(&identity) {
            Some(revision) if revision.to_string() == change.expected_version => Ok(()),
            _ => Err(ConnectorFailure::Conflict),
        }
    }

    fn result_version(&self, change: &RecordChange) -> Result<String, ConnectorFailure> {
        let generation = live_generation(&self.persist_dir);
        let delivery = load_delivery(&generation, &change.idempotency_key)?
            .ok_or(ConnectorFailure::Invalid)?;
        let identity = identity_of(change).ok();
        let record = identity.as_ref().and_then(|id| {
            delivery
                .records
                .iter()
                .find(|record| record.external_id == *id)
        });
        Ok(record
            .map(|record| record.source_version.clone())
            .unwrap_or_else(|| delivery.content_digest.clone()))
    }

    fn run(&mut self, change: &RecordChange) -> Result<(), ConnectorFailure> {
        let op = change.change.get("op").map(String::as_str).unwrap_or("");
        match op {
            "join" => {
                let identity = identity_of(change)?;
                let rules = self.rules.clone();
                let now = self.now();
                self.shard
                    .join(&identity, self.map, &mut self.empires, |_| {
                        GameState::with_config(now, rules.clone())
                    })
                    .map_err(map_shard)?;
            }
            "intent" => {
                let identity = identity_of(change)?;
                let intent = parse_intent(change)?;
                let cmd = ShardCommand {
                    identity: identity.clone(),
                    idempotency_id: change.idempotency_key.clone(),
                    intent,
                    client_now_unix: change
                        .change
                        .get("client_now_unix")
                        .and_then(|text| text.parse().ok()),
                };
                submit(&mut self.shard, &mut self.empires, cmd)?;
            }
            "launch_raid" => {
                let identity = identity_of(change)?;
                let cmd = ShardRaid {
                    identity: identity.clone(),
                    idempotency_id: change.idempotency_key.clone(),
                    ships: parse_ships(change)?,
                    dest: parse_coord(change, "dest")?,
                    speed_percent: change
                        .change
                        .get("speed_percent")
                        .and_then(|text| text.parse().ok())
                        .unwrap_or(100),
                };
                self.shard
                    .launch_raid(&mut self.empires, cmd)
                    .map_err(map_shard)?;
            }
            "settle" => self.shard.settle(&mut self.empires),
            _ => return Err(ConnectorFailure::Invalid),
        }
        Ok(())
    }
}

impl<P: DeliveryPlane> Connector for ShardHost<P> {
    fn check(&mut self) -> Result<(), ConnectorFailure> {
        std::fs::create_dir_all(&self.persist_dir).map_err(|_| ConnectorFailure::Unreachable)?;
        Ok(())
    }

    fn apply(&mut self, change: &RecordChange) -> Result<String, ConnectorFailure> {
        if !line_token(&change.idempotency_key) {
            return Err(ConnectorFailure::Invalid);
        }
        self.drain_pending()?;
        let live = live_generation(&self.persist_dir);
        if delivery_path(&live, &change.idempotency_key).exists() {
            let stored = read_digests(&live.join("changes"))?;
            if let Some(digest) = stored.get(&change.idempotency_key)
                && digest != &change_digest(change)
            {
                return Err(ConnectorFailure::Conflict);
            }
            if !read_lines(&live.join("acked"))?
                .iter()
                .any(|id| id == &change.idempotency_key)
            {
                self.publish_views(&change.idempotency_key)?;
            }
            return self.result_version(change);
        }
        self.reject_stale_version(change)?;
        let backup = self.checkpoint()?;
        let committed = (|| {
            self.run(change)?;
            let actor = identity_of(change).ok();
            let delivery = self.build_delivery(&change.idempotency_key, actor.as_deref())?;
            let digest = change_digest(change);
            self.commit_generation(Some((
                change.idempotency_key.as_str(),
                &delivery,
                digest.as_str(),
            )))
        })();
        if let Err(err) = committed {
            if delivery_path(&live_generation(&self.persist_dir), &change.idempotency_key).exists()
            {
                self.load_generation()?;
            } else {
                self.revert(backup)?;
            }
            return Err(err);
        }
        self.publish_views(&change.idempotency_key)?;
        self.result_version(change)
    }
}

fn rules() -> BalanceConfig {
    let mut cfg = BalanceConfig::default();
    cfg.raid.enabled = false;
    cfg.moons.form_chance_per_debris = 0.0;
    cfg
}

fn submit(
    shard: &mut ShardSession<HostClock>,
    empires: &mut BTreeMap<String, GameState>,
    cmd: ShardCommand,
) -> Result<(), ConnectorFailure> {
    let identity = cmd.identity.clone();
    let state = empires
        .get_mut(&identity)
        .ok_or(ConnectorFailure::Invalid)?;
    match shard.submit(state, cmd.clone()) {
        Err(ShardError::Unsettled { .. }) => {
            shard.settle(empires);
            let state = empires
                .get_mut(&identity)
                .ok_or(ConnectorFailure::Invalid)?;
            shard.submit(state, cmd).map_err(map_shard)
        }
        other => other.map_err(map_shard),
    }
}

fn map_shard(err: ShardError) -> ConnectorFailure {
    match err {
        ShardError::ShardFull { .. }
        | ShardError::NewbieProtected { .. }
        | ShardError::Intent(_)
        | ShardError::AlreadySeated { .. }
        | ShardError::AttackCapped { .. }
        | ShardError::SameEmpire
        | ShardError::NotATenantHome { .. }
        | ShardError::Occupied { .. } => ConnectorFailure::Conflict,
        _ => ConnectorFailure::Invalid,
    }
}

fn change_digest(change: &RecordChange) -> String {
    let mut payload = BTreeMap::from([("key".into(), change.key.clone())]);
    payload.extend(change.change.clone());
    digest_properties(&payload)
}

fn identity_of(change: &RecordChange) -> Result<String, ConnectorFailure> {
    let identity = change.key.clone();
    if let Some(claimed) = change.change.get("identity")
        && claimed != &identity
    {
        return Err(ConnectorFailure::Invalid);
    }
    if !identity_round_trips(&identity) {
        return Err(ConnectorFailure::Invalid);
    }
    Ok(identity)
}

fn identity_round_trips(identity: &str) -> bool {
    line_token(identity) && identity != ".." && !identity.contains(':') && !identity.contains('#')
}

fn line_token(text: &str) -> bool {
    if text.is_empty() || text != text.trim() {
        return false;
    }
    if text.contains('/') || text.contains('\\') || text.chars().any(char::is_control) {
        return false;
    }
    let encoded = [text, "peer"].join("\n");
    let decoded: Vec<&str> = encoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    decoded.len() == 2 && decoded[0] == text
}

fn parse_intent(change: &RecordChange) -> Result<Intent, ConnectorFailure> {
    let kind = change.change.get("kind").map(String::as_str).unwrap_or("");
    match kind {
        "build" => Ok(Intent::Build {
            planet: parse_coord(change, "planet")?,
            building: BuildingKind::from_key(
                change
                    .change
                    .get("building")
                    .map(String::as_str)
                    .unwrap_or(""),
            )
            .ok_or(ConnectorFailure::Invalid)?,
            actor: Actor::Player,
        }),
        "cheat" => Err(ConnectorFailure::Conflict),
        _ => Err(ConnectorFailure::Invalid),
    }
}

fn parse_coord(change: &RecordChange, field: &str) -> Result<Coord, ConnectorFailure> {
    Coord::from_key(change.change.get(field).map(String::as_str).unwrap_or(""))
        .ok_or(ConnectorFailure::Invalid)
}

fn parse_ships(change: &RecordChange) -> Result<Fleet, ConnectorFailure> {
    let mut fleet = Fleet::default();
    let spec = change.change.get("ships").map(String::as_str).unwrap_or("");
    if spec.is_empty() {
        return Err(ConnectorFailure::Invalid);
    }
    for part in spec.split(',') {
        let (kind, count) = part.split_once('=').ok_or(ConnectorFailure::Invalid)?;
        let kind = ShipKind::from_key(kind.trim()).ok_or(ConnectorFailure::Invalid)?;
        let count: u32 = count
            .trim()
            .parse()
            .map_err(|_| ConnectorFailure::Invalid)?;
        fleet.add(kind, count);
    }
    Ok(fleet)
}

fn view_properties(view: &ShardView) -> BTreeMap<String, String> {
    let mut properties = BTreeMap::from([
        ("identity".into(), view.identity.clone()),
        ("at".into(), view.at.to_string()),
        ("home".into(), view.home.to_string()),
        ("fleet".into(), fleet_spec(&view.fleet)),
        ("iron".into(), format!("{:.0}", view.resources.iron)),
    ]);
    if let Some(inbound) = view.inbound.first() {
        properties.insert("inbound_arrives_at".into(), inbound.arrives_at.to_string());
    }
    if let Some(aftermath) = view.raids.first() {
        properties.insert(
            "raid_plunder_iron".into(),
            format!("{:.0}", aftermath.plunder.iron),
        );
        properties.insert("raid_lost".into(), fleet_spec(&aftermath.lost));
    }
    properties
}

fn digest_properties(properties: &BTreeMap<String, String>) -> String {
    match serde_json::to_vec(properties) {
        Ok(bytes) => format!("sha256:{:x}", Sha256::digest(bytes)),
        Err(_) => format!(
            "sha256:{:x}",
            Sha256::digest(
                properties
                    .get("identity")
                    .map(String::as_bytes)
                    .unwrap_or(b"")
            )
        ),
    }
}

/// OCC token ignores clock catch-up (`at`, accrued `iron`) so one tenant's
/// later apply does not stale every other tenant's `expected_version`.
fn version_digest(properties: &BTreeMap<String, String>) -> String {
    let mut versioned = properties.clone();
    versioned.remove("at");
    versioned.remove("iron");
    digest_properties(&versioned)
}

fn view_record(
    source_instance: &str,
    identity: String,
    properties: BTreeMap<String, String>,
    observed_at_ms: i64,
    revision: u64,
) -> SourceRecord {
    let payload_digest = digest_properties(&properties);
    SourceRecord {
        source: SOURCE.into(),
        source_instance: source_instance.into(),
        external_id: identity.clone(),
        source_version: revision.to_string(),
        type_name: RECORD_KIND.into(),
        display_name: identity,
        payload_digest,
        properties,
        deleted: false,
        observed_at_ms,
        source_sequence: None,
    }
}

fn fleet_spec(fleet: &Fleet) -> String {
    let parts: Vec<String> = ShipKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let n = fleet.get(kind);
            (n > 0).then(|| format!("{}={n}", kind.as_key()))
        })
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(",")
    }
}

fn tenant_save(generation: &Path, identity: &str) -> PathBuf {
    generation
        .join("tenants")
        .join(format!("{identity}.sqlite"))
}

fn delivery_path(generation: &Path, id: &str) -> PathBuf {
    generation.join("deliveries").join(format!("{id}.json"))
}

fn live_generation(persist_dir: &Path) -> PathBuf {
    let current = persist_dir.join("current");
    if current.join("identities").exists() {
        current
    } else if persist_dir.join("prev").join("identities").exists() {
        persist_dir.join("prev")
    } else {
        current
    }
}

fn load_delivery(generation: &Path, id: &str) -> Result<Option<WebhookDelivery>, ConnectorFailure> {
    let path = delivery_path(generation, id);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|_| ConnectorFailure::Unreachable)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ConnectorFailure::Invalid)
}

fn previous_cursor(persist_dir: &Path) -> Option<String> {
    fs::read_to_string(live_generation(persist_dir).join("cursor"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn read_identities(path: &Path) -> Result<Vec<String>, ConnectorFailure> {
    read_lines(path)
}

fn read_revisions(path: &Path) -> Result<BTreeMap<String, u64>, ConnectorFailure> {
    let mut revisions = BTreeMap::new();
    for line in read_lines(path)? {
        let (identity, revision) = line.split_once('\t').ok_or(ConnectorFailure::Invalid)?;
        let revision = revision.parse().map_err(|_| ConnectorFailure::Invalid)?;
        revisions.insert(identity.to_string(), revision);
    }
    Ok(revisions)
}

fn read_digests(path: &Path) -> Result<BTreeMap<String, String>, ConnectorFailure> {
    let mut digests = BTreeMap::new();
    for line in read_lines(path)? {
        let (identity, digest) = line.split_once('\t').ok_or(ConnectorFailure::Invalid)?;
        digests.insert(identity.to_string(), digest.to_string());
    }
    Ok(digests)
}

fn read_lines(path: &Path) -> Result<Vec<String>, ConnectorFailure> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|_| ConnectorFailure::Unreachable)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn delivery_expired(delivery: &WebhookDelivery, now_unix: u64) -> bool {
    let now_ms = i64::try_from(now_unix.saturating_mul(1000)).unwrap_or(i64::MAX);
    now_ms >= delivery.not_after_ms
}

struct HostCheckpoint {
    empires: BTreeMap<String, GameState>,
    bookkeeping: Vec<u8>,
    commons: Vec<u8>,
    revisions: BTreeMap<String, u64>,
    digests: BTreeMap<String, String>,
}

fn resign_delivery(
    delivery: &WebhookDelivery,
    now_unix: u64,
    signing_seed: &[u8; 32],
) -> Result<WebhookDelivery, ConnectorFailure> {
    let now_ms = i64::try_from(now_unix.saturating_mul(1000)).unwrap_or(i64::MAX);
    let unsigned = WebhookDelivery {
        contract_version: delivery.contract_version.clone(),
        delivery_id: delivery.delivery_id.clone(),
        namespace: delivery.namespace.clone(),
        producer_identity: delivery.producer_identity.clone(),
        source: delivery.source.clone(),
        source_instance: delivery.source_instance.clone(),
        type_digest: delivery.type_digest.clone(),
        current_cursor: delivery.current_cursor.clone(),
        proposed_next_cursor: delivery.proposed_next_cursor.clone(),
        signed_at_ms: delivery.signed_at_ms,
        not_after_ms: now_ms.saturating_add(60_000),
        records: delivery.records.clone(),
        content_digest: String::new(),
        signature: None,
    };
    sign_delivery(unsigned, &signing_key(signing_seed), KEY_ID)
}

fn copy_tree_files(from: &Path, to: &Path) -> Result<(), ConnectorFailure> {
    if !from.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(to).map_err(|_| ConnectorFailure::Unreachable)?;
    for entry in fs::read_dir(from).map_err(|_| ConnectorFailure::Unreachable)? {
        let entry = entry.map_err(|_| ConnectorFailure::Unreachable)?;
        if entry
            .file_type()
            .map_err(|_| ConnectorFailure::Unreachable)?
            .is_file()
        {
            fs::copy(entry.path(), to.join(entry.file_name()))
                .map_err(|_| ConnectorFailure::Unreachable)?;
        }
    }
    Ok(())
}

fn copy_if_exists(from: &Path, to: &Path) -> Result<(), ConnectorFailure> {
    if from.exists() {
        fs::copy(from, to).map_err(|_| ConnectorFailure::Unreachable)?;
    }
    Ok(())
}

fn acquire_lock(dir: &Path) -> Result<fs::File, ConnectorFailure> {
    fs::create_dir_all(dir).map_err(|_| ConnectorFailure::Unreachable)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"))
        .map_err(|_| ConnectorFailure::Unreachable)?;
    file.try_lock().map_err(|_| ConnectorFailure::Conflict)?;
    Ok(file)
}

fn sync_dir(path: &Path) -> Result<(), ConnectorFailure> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| ConnectorFailure::Unreachable)
}

fn sync_tree(dir: &Path) -> Result<(), ConnectorFailure> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|_| ConnectorFailure::Unreachable)? {
        let path = entry.map_err(|_| ConnectorFailure::Unreachable)?.path();
        if path.is_dir() {
            sync_tree(&path)?;
        } else {
            fs::File::open(&path)
                .and_then(|file| file.sync_all())
                .map_err(|_| ConnectorFailure::Unreachable)?;
        }
    }
    sync_dir(dir)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConnectorFailure> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| ConnectorFailure::Unreachable)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|_| ConnectorFailure::Unreachable)?;
        file.write_all(bytes)
            .map_err(|_| ConnectorFailure::Unreachable)?;
        file.sync_all().map_err(|_| ConnectorFailure::Unreachable)?;
    }
    fs::rename(&tmp, path).map_err(|_| ConnectorFailure::Unreachable)?;
    if let Some(parent) = path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

#[cfg(test)]
impl<P> ShardHost<P> {
    pub(crate) fn park(&mut self, identity: &str, kind: ShipKind, n: u32) {
        self.empires.get_mut(identity).unwrap().fleet.add(kind, n);
    }

    pub(crate) fn stock_iron(&mut self, identity: &str, iron: f64) {
        self.empires.get_mut(identity).unwrap().resources.iron = iron;
    }

    pub(crate) fn stock_deuterium(&mut self, identity: &str, deuterium: f64) {
        self.empires.get_mut(identity).unwrap().resources.deuterium = deuterium;
    }

    pub(crate) fn seated(&self, identity: &str) -> bool {
        self.empires.contains_key(identity)
    }

    pub(crate) fn home(&self, identity: &str) -> Coord {
        self.empires[identity].home
    }

    pub(crate) fn last_tick(&self, identity: &str) -> u64 {
        self.empires[identity].last_tick_unix
    }

    pub(crate) fn build_queue_len(&self, identity: &str) -> usize {
        self.empires[identity].build_queue.len()
    }

    pub(crate) fn tenant_iron(&self, identity: &str) -> f64 {
        self.empires[identity].resources.iron
    }

    pub(crate) fn mission_cargo_iron(&self, identity: &str) -> Option<f64> {
        self.empires[identity]
            .missions
            .first()
            .map(|mission| mission.cargo.iron)
    }
}

#[cfg(test)]
impl ShardHost<crate::webhook::MemoryPlane> {
    pub(crate) fn object(&self, identity: &str) -> Option<&BTreeMap<String, String>> {
        let source_id = sekai_source_sdk::source_id(SOURCE, &self.source_instance, identity);
        self.plane.objects.get(&source_id)
    }
}
