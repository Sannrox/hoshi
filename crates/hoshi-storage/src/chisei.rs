//! Optional in-process Chisei-shaped backend. No network.

use super::{Storage, cleanup_sqlite};
use crate::save::{SaveError, load_state, open_save, save_state};
use hoshi_core::BalanceConfig;
use hoshi_core::Coord;
use hoshi_core::GameState;
use hoshi_core::{CHISEI_API_VERSION, ONTOLOGY_PACKAGE, bundled_ontology, validate_ontology};
use hoshi_core::{Fleet, ShipKind};
use hoshi_core::{Intent, IntentError, Proposal, apply, propose};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const HOME_TENANT: &str = "player";

#[derive(Debug, Clone, Default, Deserialize)]
struct GraphFile {
    #[serde(default)]
    api: String,
    #[serde(default)]
    package: String,
    #[serde(default = "default_tenant")]
    home_tenant: String,
    #[serde(default)]
    object: Vec<GraphObject>,
    #[serde(default)]
    link: Vec<GraphLink>,
    #[serde(default)]
    marking: Vec<GraphMarking>,
    #[serde(default)]
    alliance: Vec<GraphAlliance>,
    #[serde(default)]
    spy: Vec<GraphSpy>,
    #[serde(default)]
    applied: Vec<String>,
}

fn default_tenant() -> String {
    HOME_TENANT.to_string()
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct GraphObject {
    pub kind: String,
    pub key: String,
    #[serde(default = "default_tenant")]
    pub tenant: String,
    #[serde(default)]
    pub props: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct GraphLink {
    pub rel: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct GraphMarking {
    object: String,
    key: String,
    value: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct GraphAlliance {
    a: String,
    b: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct GraphSpy {
    viewer: String,
    planet: String,
}

/// Directory `*.chisei` save: SQLite codec plus an object graph.
pub struct ChiseiStorage {
    dir: PathBuf,
    graph: GraphFile,
    committed: u64,
}

impl ChiseiStorage {
    pub fn open(path: &Path) -> Result<Self, SaveError> {
        validate_ontology(bundled_ontology()).map_err(|e| SaveError::Backend(e.to_string()))?;
        if path.exists() && path.is_file() {
            return Err(SaveError::Backend(
                "a .chisei save is a directory, not a file".into(),
            ));
        }
        std::fs::create_dir_all(path)?;
        let committed = read_head(path)?;
        let mut store = Self {
            dir: path.to_path_buf(),
            graph: GraphFile {
                api: CHISEI_API_VERSION.into(),
                package: ONTOLOGY_PACKAGE.into(),
                home_tenant: HOME_TENANT.into(),
                ..GraphFile::default()
            },
            committed,
        };
        if committed > 0 {
            let graph_path = store.graph_file(committed);
            if !graph_path.exists() {
                return Err(SaveError::Corrupt("chisei graph"));
            }
            let text = std::fs::read_to_string(&graph_path)?;
            store.graph = toml::from_str(&text).map_err(|_| SaveError::Corrupt("chisei graph"))?;
        }
        let ontology_path = store.dir.join("ontology.json");
        if !ontology_path.exists() {
            std::fs::write(&ontology_path, bundled_ontology())?;
        }
        Ok(store)
    }

    fn head_path(&self) -> PathBuf {
        self.dir.join("HEAD")
    }

    fn state_file(&self, rev: u64) -> PathBuf {
        self.dir.join(format!("state.{rev}.sqlite"))
    }

    fn graph_file(&self, rev: u64) -> PathBuf {
        self.dir.join(format!("graph.{rev}.toml"))
    }

    fn tenant_path(&self, tenant: &str) -> Result<PathBuf, SaveError> {
        valid_tenant(tenant)?;
        Ok(self.dir.join("tenants").join(format!("{tenant}.sqlite")))
    }

    pub fn api_version(&self) -> &str {
        if self.graph.api.is_empty() {
            CHISEI_API_VERSION
        } else {
            &self.graph.api
        }
    }

    pub fn objects(&self) -> &[GraphObject] {
        &self.graph.object
    }

    pub fn links(&self) -> &[GraphLink] {
        &self.graph.link
    }

    pub fn upsert_object(&mut self, object: GraphObject) {
        if let Some(existing) = self
            .graph
            .object
            .iter_mut()
            .find(|row| row.kind == object.kind && row.key == object.key)
        {
            *existing = object;
        } else {
            self.graph.object.push(object);
        }
    }

    /// Place a foreign tenant planet on the graph (demo / fog fixtures).
    pub fn upsert_planet(&mut self, tenant: &str, coord: Coord) {
        self.upsert_object(GraphObject {
            kind: "Planet".into(),
            key: scoped(tenant, &coord.to_string()),
            tenant: tenant.into(),
            props: String::new(),
        });
    }

    pub fn upsert_link(&mut self, link: GraphLink) {
        if !self.graph.link.iter().any(|row| row == &link) {
            self.graph.link.push(link);
        }
    }

    pub fn set_alliance(&mut self, a: &str, b: &str, allied: bool) {
        let (a, b) = ordered_pair(a, b);
        self.graph.alliance.retain(|row| {
            let (x, y) = ordered_pair(&row.a, &row.b);
            x != a || y != b
        });
        self.graph
            .link
            .retain(|row| !(row.rel == "allied-with" && pair_matches(&row.from, &row.to, &a, &b)));
        if allied {
            self.graph.alliance.push(GraphAlliance {
                a: a.clone(),
                b: b.clone(),
            });
            self.upsert_link(GraphLink {
                rel: "allied-with".into(),
                from: format!("Player:{a}"),
                to: format!("Player:{b}"),
            });
        }
    }

    pub fn grant_spy(&mut self, viewer: &str, planet: &str) {
        if !self
            .graph
            .spy
            .iter()
            .any(|row| row.viewer == viewer && row.planet == planet)
        {
            self.graph.spy.push(GraphSpy {
                viewer: viewer.into(),
                planet: planet.into(),
            });
        }
        let object = format!("Planet:{planet}");
        if !self
            .graph
            .marking
            .iter()
            .any(|row| row.object == object && row.key == "cleared-for" && row.value == viewer)
        {
            self.graph.marking.push(GraphMarking {
                object,
                key: "cleared-for".into(),
                value: viewer.into(),
            });
        }
    }

    pub fn persist_graph(&mut self) -> Result<(), SaveError> {
        if self.committed == 0 {
            return Err(SaveError::Backend(
                "save the home empire before adding tenants".into(),
            ));
        }
        let next = self.committed.saturating_add(1);
        if self.committed > 0 {
            let prev = self.state_file(self.committed);
            let dest = self.state_file(next);
            if prev.exists() {
                std::fs::copy(&prev, &dest)?;
            }
        }
        self.write_graph(next)?;
        self.publish(next)?;
        Ok(())
    }

    fn write_graph(&self, rev: u64) -> Result<(), SaveError> {
        let dest = self.graph_file(rev);
        let tmp = dest.with_extension("toml.tmp");
        std::fs::write(&tmp, encode_graph(&self.graph))?;
        std::fs::rename(&tmp, dest)?;
        Ok(())
    }

    fn publish(&mut self, rev: u64) -> Result<(), SaveError> {
        let dest = self.head_path();
        let tmp = self.dir.join("HEAD.tmp");
        std::fs::write(&tmp, format!("{rev}\n"))?;
        std::fs::rename(&tmp, dest)?;
        self.prune(rev);
        self.committed = rev;
        Ok(())
    }

    fn prune(&self, keep: u64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(rest) = name.strip_prefix("state.")
                && let Some(num) = rest.strip_suffix(".sqlite")
                && let Ok(rev) = num.parse::<u64>()
                && rev != keep
            {
                let _ = std::fs::remove_file(entry.path());
                cleanup_sqlite(&entry.path());
            }
            if let Some(rest) = name.strip_prefix("graph.")
                && let Some(num) = rest.strip_suffix(".toml")
                && let Ok(rev) = num.parse::<u64>()
                && rev != keep
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Planet keys `viewer` may read (own, allied, or spy-cleared).
    pub fn visible_planets(&self, viewer: &str) -> Vec<String> {
        self.graph
            .object
            .iter()
            .filter(|obj| obj.kind == "Planet" && self.can_see(viewer, obj))
            .map(|obj| obj.key.clone())
            .collect()
    }

    /// Own, allied, or spy-cleared planet coordinates.
    pub fn visible_coords(&self, viewer: &str) -> Vec<Coord> {
        self.graph
            .object
            .iter()
            .filter(|obj| obj.kind == "Planet" && self.can_see(viewer, obj))
            .filter_map(|obj| planet_coord(&obj.tenant, &obj.key))
            .collect()
    }

    /// Planets owned by tenants allied with `viewer`.
    pub fn allied_planets(&self, viewer: &str) -> Vec<Coord> {
        self.graph
            .object
            .iter()
            .filter(|obj| {
                obj.kind == "Planet" && obj.tenant != viewer && self.allied(viewer, &obj.tenant)
            })
            .filter_map(|obj| planet_coord(&obj.tenant, &obj.key))
            .collect()
    }

    fn can_see(&self, viewer: &str, obj: &GraphObject) -> bool {
        if obj.tenant == viewer {
            return true;
        }
        if self.allied(viewer, &obj.tenant) {
            return true;
        }
        obj.kind == "Planet"
            && self
                .graph
                .spy
                .iter()
                .any(|row| row.viewer == viewer && row.planet == obj.key)
    }

    fn allied(&self, a: &str, b: &str) -> bool {
        let (a, b) = ordered_pair(a, b);
        self.graph.alliance.iter().any(|row| {
            let (x, y) = ordered_pair(&row.a, &row.b);
            x == a && y == b
        })
    }

    pub fn put_tenant(&mut self, tenant: &str, state: &GameState) -> Result<(), SaveError> {
        state
            .require_fair_play()
            .map_err(|e| SaveError::Backend(e.to_string()))?;
        valid_tenant(tenant)?;
        std::fs::create_dir_all(self.dir.join("tenants"))?;
        persist_sqlite(&self.tenant_path(tenant)?, state)?;
        project_state(tenant, state, &mut self.graph);
        self.persist_graph()
    }

    /// Propose, apply, persist. A refused proposal leaves the save unchanged.
    pub fn apply_intent(
        &mut self,
        state: &mut GameState,
        intent: Intent,
        now_unix: u64,
    ) -> Result<Proposal, IntentError> {
        let proposal = propose(state, &intent, now_unix)?;
        if self.graph.applied.iter().any(|id| id == &proposal.id) {
            return Ok(proposal);
        }
        apply(state, &proposal, now_unix)?;
        self.graph.applied.push(proposal.id.clone());
        self.save(state)
            .map_err(|e| IntentError::Save(e.to_string()))?;
        Ok(proposal)
    }

    fn project_home(&mut self, state: &GameState) {
        project_state(HOME_TENANT, state, &mut self.graph);
        self.graph.api = CHISEI_API_VERSION.into();
        self.graph.package = ONTOLOGY_PACKAGE.into();
        self.graph.home_tenant = HOME_TENANT.into();
    }
}

impl Storage for ChiseiStorage {
    fn load(&self, config: BalanceConfig) -> Result<GameState, SaveError> {
        if self.committed == 0 {
            return Err(SaveError::Corrupt("missing save"));
        }
        let path = self.state_file(self.committed);
        if !path.exists() {
            return Err(SaveError::Corrupt("missing save"));
        }
        let conn = open_save(&path)?;
        load_state(&conn, config)
    }

    fn save(&mut self, state: &GameState) -> Result<(), SaveError> {
        if self.committed > 0 {
            let previous = self.load(state.config.clone())?;
            if !state.cheats.preserves(&previous.cheats) {
                return Err(SaveError::Corrupt("stale cheat provenance"));
            }
        }
        let next = self.committed.saturating_add(1);
        persist_sqlite(&self.state_file(next), state)?;
        self.project_home(state);
        if let Err(err) = self.write_graph(next) {
            let _ = std::fs::remove_file(self.state_file(next));
            cleanup_sqlite(&self.state_file(next));
            return Err(err);
        }
        self.publish(next)?;
        Ok(())
    }

    fn is_empty(&self) -> Result<bool, SaveError> {
        Ok(self.committed == 0)
    }
}

fn read_head(dir: &Path) -> Result<u64, SaveError> {
    let path = dir.join("HEAD");
    if !path.exists() {
        return Ok(0);
    }
    let text = std::fs::read_to_string(path)?;
    text.trim()
        .parse()
        .map_err(|_| SaveError::Corrupt("chisei head"))
}

fn valid_tenant(id: &str) -> Result<(), SaveError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(SaveError::Backend("invalid tenant id".into()));
    }
    Ok(())
}

fn persist_sqlite(path: &Path, state: &GameState) -> Result<(), SaveError> {
    let tmp = path.with_extension("sqlite.tmp");
    let _ = std::fs::remove_file(&tmp);
    cleanup_sqlite(&tmp);
    {
        let mut conn = open_save(&tmp)?;
        save_state(&mut conn, state)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    std::fs::rename(&tmp, path)?;
    cleanup_sqlite(&tmp);
    Ok(())
}

fn project_state(tenant: &str, state: &GameState, graph: &mut GraphFile) {
    let stale: BTreeSet<String> = graph
        .object
        .iter()
        .filter(|obj| obj.tenant == tenant)
        .map(|obj| format!("{}:{}", obj.kind, obj.key))
        .collect();
    graph.object.retain(|obj| obj.tenant != tenant);
    graph
        .link
        .retain(|link| !stale.contains(&link.from) && !stale.contains(&link.to));
    graph.marking.retain(|row| !stale.contains(&row.object));
    graph.object.push(GraphObject {
        kind: "Player".into(),
        key: tenant.into(),
        tenant: tenant.into(),
        props: format!("tick={}", state.last_tick_unix),
    });
    push_planet(graph, tenant, state.home, state.home_kind.as_key());
    graph.link.push(GraphLink {
        rel: "owns".into(),
        from: format!("Player:{tenant}"),
        to: format!("Planet:{}", scoped(tenant, &state.home.to_string())),
    });
    for colony in &state.colonies {
        push_planet(graph, tenant, colony.coord, colony.kind.as_key());
        graph.link.push(GraphLink {
            rel: "owns".into(),
            from: format!("Player:{tenant}"),
            to: format!("Planet:{}", scoped(tenant, &colony.coord.to_string())),
        });
    }
    if state.fleet.total() > 0 {
        let key = scoped(tenant, &format!("parked:{}", state.home));
        graph.object.push(GraphObject {
            kind: "Fleet".into(),
            key: key.clone(),
            tenant: tenant.into(),
            props: fleet_prop(&state.fleet),
        });
        graph.link.push(GraphLink {
            rel: "stationed-at".into(),
            from: format!("Fleet:{key}"),
            to: format!("Planet:{}", scoped(tenant, &state.home.to_string())),
        });
    }
    for mission in &state.missions {
        let key = scoped(tenant, &format!("mission:{}", mission.id));
        graph.object.push(GraphObject {
            kind: "Fleet".into(),
            key: key.clone(),
            tenant: tenant.into(),
            props: fleet_prop(&mission.ships),
        });
        graph.link.push(GraphLink {
            rel: "stationed-at".into(),
            from: format!("Fleet:{key}"),
            to: format!("Planet:{}", scoped(tenant, &mission.dest.to_string())),
        });
    }
    for outpost in &state.outposts {
        graph.object.push(GraphObject {
            kind: "Outpost".into(),
            key: scoped(tenant, &outpost.coord.to_string()),
            tenant: tenant.into(),
            props: format!("garrison={}", outpost.garrison),
        });
    }
    for faction in state.config.diplomacy.iter() {
        graph.object.push(GraphObject {
            kind: "Faction".into(),
            key: scoped(tenant, &faction.id),
            tenant: tenant.into(),
            props: faction.name.clone(),
        });
    }
    for obj in graph
        .object
        .iter()
        .filter(|o| o.kind == "Planet" && o.tenant == tenant)
    {
        let object = format!("Planet:{}", obj.key);
        if !graph
            .marking
            .iter()
            .any(|row| row.object == object && row.key == "tenant")
        {
            graph.marking.push(GraphMarking {
                object,
                key: "tenant".into(),
                value: tenant.into(),
            });
        }
    }
}

fn push_planet(graph: &mut GraphFile, tenant: &str, coord: Coord, kind: &str) {
    graph.object.push(GraphObject {
        kind: "Planet".into(),
        key: scoped(tenant, &coord.to_string()),
        tenant: tenant.into(),
        props: format!("kind={kind}"),
    });
}

fn scoped(tenant: &str, key: &str) -> String {
    format!("{tenant}:{key}")
}

fn planet_coord(tenant: &str, key: &str) -> Option<Coord> {
    let rest = key.strip_prefix(&format!("{tenant}:")).unwrap_or(key);
    Coord::from_key(rest)
}

fn fleet_prop(fleet: &Fleet) -> String {
    ShipKind::ALL
        .iter()
        .filter_map(|&kind| {
            let n = fleet.get(kind);
            (n > 0).then(|| format!("{}:{n}", kind.as_key()))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn pair_matches(from: &str, to: &str, a: &str, b: &str) -> bool {
    let fa = from.rsplit(':').next().unwrap_or(from);
    let tb = to.rsplit(':').next().unwrap_or(to);
    let (x, y) = ordered_pair(fa, tb);
    x == a && y == b
}

fn encode_graph(graph: &GraphFile) -> String {
    let mut out = format!(
        "api = \"{}\"\npackage = \"{}\"\nhome_tenant = \"{}\"\n",
        esc(&graph.api),
        esc(&graph.package),
        esc(&graph.home_tenant)
    );
    if !graph.applied.is_empty() {
        let items: Vec<String> = graph
            .applied
            .iter()
            .map(|id| format!("\"{}\"", esc(id)))
            .collect();
        out.push_str(&format!("applied = [{}]\n", items.join(", ")));
    }
    for obj in &graph.object {
        out.push_str(&format!(
            "\n[[object]]\nkind = \"{}\"\nkey = \"{}\"\ntenant = \"{}\"\nprops = \"{}\"\n",
            esc(&obj.kind),
            esc(&obj.key),
            esc(&obj.tenant),
            esc(&obj.props)
        ));
    }
    for link in &graph.link {
        out.push_str(&format!(
            "\n[[link]]\nrel = \"{}\"\nfrom = \"{}\"\nto = \"{}\"\n",
            esc(&link.rel),
            esc(&link.from),
            esc(&link.to)
        ));
    }
    for marking in &graph.marking {
        out.push_str(&format!(
            "\n[[marking]]\nobject = \"{}\"\nkey = \"{}\"\nvalue = \"{}\"\n",
            esc(&marking.object),
            esc(&marking.key),
            esc(&marking.value)
        ));
    }
    for row in &graph.alliance {
        out.push_str(&format!(
            "\n[[alliance]]\na = \"{}\"\nb = \"{}\"\n",
            esc(&row.a),
            esc(&row.b)
        ));
    }
    for row in &graph.spy {
        out.push_str(&format!(
            "\n[[spy]]\nviewer = \"{}\"\nplanet = \"{}\"\n",
            esc(&row.viewer),
            esc(&row.planet)
        ));
    }
    out
}

fn esc(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
pub(crate) fn unique_chisei(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "hoshi-storage-{tag}-{}-{nanos}.chisei",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_storage_contract;
    use hoshi_core::Actor;
    use hoshi_core::BuildingKind;
    use hoshi_core::{Clock, FakeClock};
    use std::collections::BTreeSet;

    fn cleanup_dir(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(crate::save_lock_path(path));
    }

    #[test]
    fn chisei_backend_meets_storage_contract() {
        let path = unique_chisei("contract");
        let mut store = ChiseiStorage::open(&path).unwrap();
        assert_storage_contract(&mut store);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn save_writes_player_planet_fleet_faction_outpost_and_links() {
        let path = unique_chisei("graph");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.fleet.add(ShipKind::SmallCargo, 2);
        store.save(&state).unwrap();
        let kinds: BTreeSet<_> = store.objects().iter().map(|o| o.kind.as_str()).collect();
        for need in ["Player", "Planet", "Fleet", "Faction", "Outpost"] {
            assert!(kinds.contains(need), "missing {need} in {kinds:?}");
        }
        assert!(
            store
                .links()
                .iter()
                .any(|l| l.rel == "owns" && l.from == "Player:player")
        );
        assert!(store.links().iter().any(|l| l.rel == "stationed-at"));
        assert_eq!(store.api_version(), CHISEI_API_VERSION);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn save_keeps_classified_object_ids_out_of_the_shared_graph() {
        let path = unique_chisei("marking");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 9,
            target: Coord::HOME,
            created_at: 1_000,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 1.0,
            tier: "fleet".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 1_000,
            arrives_at: 0,
            confidence: 1.0,
            deceived: false,
        });
        state
            .classify(
                hoshi_core::Actor::Player,
                hoshi_core::ClassifiedKind::Report,
                "9",
                hoshi_core::Classification::Secret,
                [hoshi_core::COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        store.save(&state).unwrap();
        assert_eq!(
            state
                .marking_of(hoshi_core::ClassifiedKind::Report, "9")
                .classification,
            hoshi_core::Classification::Secret
        );
        assert!(store.objects().iter().all(|o| !matches!(
            o.kind.as_str(),
            "Marking" | "Compartment" | "Clearance" | "SharingAgreement"
        )));
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn put_tenant_projects_a_second_player() {
        let path = unique_chisei("tenant");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let alpha = GameState::with_config(1_000, cfg.clone());
        store.save(&alpha).unwrap();
        let bravo = GameState::with_config(2_000, cfg);
        store.put_tenant("bravo", &bravo).unwrap();
        assert!(
            store
                .objects()
                .iter()
                .any(|o| o.kind == "Player" && o.key == "bravo")
        );
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn projecting_one_tenant_keeps_the_other_tenants_links() {
        let path = unique_chisei("isolate");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut alpha = GameState::with_config(1_000, cfg.clone());
        alpha.fleet.add(ShipKind::SmallCargo, 1);
        store.save(&alpha).unwrap();
        let mut bravo = GameState::with_config(2_000, cfg);
        bravo.fleet.add(ShipKind::SmallCargo, 2);
        store.put_tenant("bravo", &bravo).unwrap();
        let bravo_links = store
            .links()
            .iter()
            .filter(|l| l.from == "Player:bravo" || l.from.starts_with("Fleet:bravo:"))
            .count();
        assert!(bravo_links > 0);
        store.save(&alpha).unwrap();
        let after = store
            .links()
            .iter()
            .filter(|l| l.from == "Player:bravo" || l.from.starts_with("Fleet:bravo:"))
            .count();
        assert_eq!(after, bravo_links);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn two_player_fog_clears_for_alliance_and_spy() {
        let path = unique_chisei("fog");
        let mut store = ChiseiStorage::open(&path).unwrap();
        store.upsert_object(GraphObject {
            kind: "Player".into(),
            key: "alpha".into(),
            tenant: "alpha".into(),
            props: String::new(),
        });
        store.upsert_object(GraphObject {
            kind: "Player".into(),
            key: "bravo".into(),
            tenant: "bravo".into(),
            props: String::new(),
        });
        store.upsert_object(GraphObject {
            kind: "Planet".into(),
            key: "1:1:1".into(),
            tenant: "alpha".into(),
            props: "kind=rocky".into(),
        });
        store.upsert_object(GraphObject {
            kind: "Planet".into(),
            key: "1:2:4".into(),
            tenant: "bravo".into(),
            props: "kind=ice".into(),
        });
        assert_eq!(store.visible_planets("alpha"), vec!["1:1:1".to_string()]);
        assert!(!store.visible_planets("alpha").contains(&"1:2:4".into()));
        store.set_alliance("alpha", "bravo", true);
        assert!(store.visible_planets("alpha").contains(&"1:2:4".into()));
        assert!(store.visible_planets("bravo").contains(&"1:1:1".into()));
        assert_eq!(
            store.allied_planets("alpha"),
            vec![Coord {
                galaxy: 1,
                system: 2,
                position: 4
            }]
        );
        assert!(store.links().iter().any(|l| l.rel == "allied-with"));
        store.set_alliance("alpha", "bravo", false);
        assert!(!store.visible_planets("alpha").contains(&"1:2:4".into()));
        store.grant_spy("alpha", "1:2:4");
        assert!(store.visible_planets("alpha").contains(&"1:2:4".into()));
        assert!(
            store
                .graph
                .marking
                .iter()
                .any(|m| m.key == "cleared-for" && m.value == "alpha")
        );
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn refused_intent_does_not_write_the_save() {
        let path = unique_chisei("refuse");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        store.save(&state).unwrap();
        let metal = state.resources.iron;
        let err = store
            .apply_intent(
                &mut state,
                Intent::Research {
                    tech: "missing-tech".into(),
                },
                1_000,
            )
            .unwrap_err();
        assert!(matches!(err, IntentError::Research(_)));
        assert_eq!(state.resources.iron, metal);
        let loaded = store.load(BalanceConfig::default()).unwrap();
        assert_eq!(loaded.resources.iron, metal);
        assert!(store.graph.applied.is_empty());
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn apply_intent_persists_a_build() {
        let path = unique_chisei("apply");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
        store
            .apply_intent(
                &mut state,
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::MetalMine,
                    actor: Actor::Player,
                },
                1_000,
            )
            .unwrap();
        let loaded = store.load(BalanceConfig::default()).unwrap();
        assert_eq!(loaded.build_queue.len(), 1);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn two_buildings_at_the_same_tick_both_apply() {
        let path = unique_chisei("two-builds");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 80_000.0;
        state.resources.silicon = 40_000.0;
        store
            .apply_intent(
                &mut state,
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::MetalMine,
                    actor: Actor::Player,
                },
                1_000,
            )
            .unwrap();
        store
            .apply_intent(
                &mut state,
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::CrystalMine,
                    actor: Actor::Player,
                },
                1_000,
            )
            .unwrap();
        assert_eq!(state.build_queue.len(), 2);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn applied_ids_reload_from_the_committed_graph() {
        let path = unique_chisei("applied");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
        let proposal = store
            .apply_intent(
                &mut state,
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::MetalMine,
                    actor: Actor::Player,
                },
                1_000,
            )
            .unwrap();
        drop(store);
        let mut again = ChiseiStorage::open(&path).unwrap();
        assert!(again.graph.applied.iter().any(|id| id == &proposal.id));
        let mut loaded = again.load(BalanceConfig::default()).unwrap();
        again
            .apply_intent(
                &mut loaded,
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::MetalMine,
                    actor: Actor::Player,
                },
                1_000,
            )
            .unwrap();
        assert_eq!(loaded.build_queue.len(), 1);
        drop(again);
        cleanup_dir(&path);
    }

    #[test]
    fn put_tenant_rejects_path_escape() {
        let path = unique_chisei("escape");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let state = GameState::with_config(1_000, BalanceConfig::default());
        let err = store.put_tenant("../state", &state).unwrap_err();
        assert!(matches!(err, SaveError::Backend(_)));
        let err = store.put_tenant("/tmp/x", &state).unwrap_err();
        assert!(matches!(err, SaveError::Backend(_)));
        let err = store.put_tenant("bravo", &state).unwrap_err();
        assert!(matches!(err, SaveError::Backend(_)));
        assert!(store.is_empty().unwrap());
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn snapshot_rejects_stale_clean_provenance() {
        let path = unique_chisei("cheat-provenance");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut state = GameState::new(100);
        store.save(&state).unwrap();
        let stale = state.clone();
        state.set_cheats_enabled(true, hoshi_core::Actor::Player, 100);
        let intent = hoshi_core::Intent::Cheat {
            code: "grant iron 100".into(),
            actor: hoshi_core::Actor::Player,
        };
        let proposal = hoshi_core::propose(&state, &intent, 100).unwrap();
        hoshi_core::apply(&mut state, &proposal, 100).unwrap();
        store.save(&state).unwrap();
        assert!(store.save(&stale).is_err());
        let loaded = store.load(BalanceConfig::default()).unwrap();
        assert!(loaded.is_modified());
        assert_eq!(loaded.cheat_audit(), state.cheat_audit());
        cleanup_dir(&path);
    }

    #[test]
    fn second_save_does_not_duplicate_fleet_links() {
        let path = unique_chisei("links");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.fleet.add(ShipKind::SmallCargo, 2);
        store.save(&state).unwrap();
        let first = store
            .links()
            .iter()
            .filter(|l| l.rel == "stationed-at")
            .count();
        store.save(&state).unwrap();
        let second = store
            .links()
            .iter()
            .filter(|l| l.rel == "stationed-at")
            .count();
        assert_eq!(first, 1);
        assert_eq!(second, 1);
        drop(store);
        cleanup_dir(&path);
    }

    #[test]
    fn load_follows_committed_head_not_a_newer_file() {
        let path = unique_chisei("head");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg.clone());
        state.resources.iron = 111.0;
        store.save(&state).unwrap();
        let rev = store.committed;
        state.resources.iron = 999.0;
        persist_sqlite(&store.state_file(rev + 1), &state).unwrap();
        drop(store);
        let again = ChiseiStorage::open(&path).unwrap();
        let loaded = again.load(cfg).unwrap();
        assert!((loaded.resources.iron - 111.0).abs() < 1e-9);
        drop(again);
        cleanup_dir(&path);
    }

    #[test]
    fn missing_committed_graph_is_corrupt() {
        let path = unique_chisei("nograph");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let state = GameState::with_config(1_000, cfg);
        store.save(&state).unwrap();
        let graph = store.graph_file(store.committed);
        drop(store);
        std::fs::remove_file(&graph).unwrap();
        match ChiseiStorage::open(&path) {
            Err(SaveError::Corrupt(_)) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("expected corrupt graph"),
        }
        cleanup_dir(&path);
    }

    /// CPU time of this thread, so sibling tests do not count against the budget.
    #[cfg(unix)]
    fn thread_cpu_time() -> std::time::Duration {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid out-pointer for the duration of the call.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
    }

    #[test]
    #[cfg(unix)]
    fn apply_save_throughput_stays_under_twelve_seconds_for_fifty() {
        let path = unique_chisei("soak");
        let mut store = ChiseiStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 500_000.0;
        state.resources.silicon = 500_000.0;
        state.resources.deuterium = 500_000.0;
        let clock = FakeClock::new(1_000);
        let started = thread_cpu_time();
        for i in 0..50u32 {
            clock.advance(60);
            let now = clock.now_unix();
            state.catch_up(now);
            let intent = if i.is_multiple_of(2) {
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::MetalMine,
                    actor: Actor::Player,
                }
            } else {
                Intent::Build {
                    planet: Coord::HOME,
                    building: BuildingKind::CrystalMine,
                    actor: Actor::Player,
                }
            };
            match store.apply_intent(&mut state, intent, now) {
                Ok(_) => {}
                Err(_) => store.save(&state).unwrap(),
            }
        }
        let elapsed = thread_cpu_time().saturating_sub(started);
        assert!(
            elapsed.as_secs_f64() < 12.0,
            "50 apply+save cycles took {elapsed:?}"
        );
        drop(store);
        cleanup_dir(&path);
    }
}
