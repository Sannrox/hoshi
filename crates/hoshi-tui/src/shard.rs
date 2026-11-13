//! Title-screen Join: a shard client, never a local save.
//!
//! Credentials live beside config (ADR 0020). Network stays in this crate.
//! `hoshi-core` is not a host and grows no sockets here.

use crate::keymap::{Binding, Keymap};
use crate::ui::chrome::{Navbar, StatusBar};
use crate::ui::{Depth, Intent as ColorIntent, Surface, Theme};
use hoshi_core::{Actor, BuildingKind, Coord, Fleet, Intent, ShardCommand, ShardView, ShipKind};
#[cfg(test)]
use hoshi_core::{
    BalanceConfig, FakeClock, GameState, ShardConfig, ShardError, ShardMap, ShardSession,
};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde::Deserialize;
#[cfg(test)]
use serde::Serialize;
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SESSION: AtomicU64 = AtomicU64::new(1);

fn session_nonce() -> String {
    let n = SESSION.fetch_add(1, Ordering::Relaxed);
    let mut buf = [0u8; 8];
    let _ = getrandom::getrandom(&mut buf);
    format!("{n:x}-{:016x}", u64::from_le_bytes(buf))
}

const ACTIONS: [&str; 3] = ["build", "research", "fleet"];
const HIDDEN_KEYS: &[&str] = &[
    "rng",
    "pirate",
    "pirate_timers",
    "signing_seed",
    "bridge_mac",
    "bookkeeping",
];

/// Host tenant identity. Never stored in `save.sqlite`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(test, derive(Serialize))]
pub struct Credentials {
    pub identity: String,
}

impl Credentials {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("shard.json")
    }

    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(dir)).ok()?;
        let creds: Self = serde_json::from_str(&text).ok()?;
        valid_identity(&creds.identity).then_some(creds)
    }

    #[cfg(test)]
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        if !valid_identity(&self.identity) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid shard identity",
            ));
        }
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        atomicwrites::AtomicFile::new(
            Self::path(dir),
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        )
        .write(|file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&text)?;
            file.sync_all()
        })
        .map_err(|error| std::io::Error::other(error.to_string()))
    }

    #[cfg(test)]
    pub fn logout(dir: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(Self::path(dir)) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

fn valid_identity(identity: &str) -> bool {
    !identity.is_empty()
        && identity == identity.trim()
        && identity != ".."
        && !identity.contains('/')
        && !identity.contains('\\')
        && !identity.contains(':')
        && !identity.contains('#')
        && !identity.chars().any(char::is_control)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Gone,
    /// A live host refused the intent. Tests construct this; production matches it.
    #[allow(dead_code)]
    Refused(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => f.write_str("host is gone"),
            Self::Refused(msg) => f.write_str(msg),
        }
    }
}

/// Talks to a shard host. The TUI never `apply`s locally on this path.
pub trait ShardClient {
    fn join(&mut self, identity: &str) -> Result<ShardView, ClientError>;
    fn submit(&mut self, cmd: ShardCommand) -> Result<ShardView, ClientError>;
}

/// Fail closed when the plane is unreachable. Does not open a save.
#[derive(Debug, Default)]
pub struct UnreachableHost;

impl ShardClient for UnreachableHost {
    fn join(&mut self, _identity: &str) -> Result<ShardView, ClientError> {
        Err(ClientError::Gone)
    }

    fn submit(&mut self, _cmd: ShardCommand) -> Result<ShardView, ClientError> {
        Err(ClientError::Gone)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardAction {
    None,
    Quit,
}

/// Client session: tenant view, intents with idempotency, disconnect overlay.
pub struct ShardPlay<C> {
    client: C,
    identity: String,
    view: Option<ShardView>,
    json: serde_json::Value,
    disconnected: bool,
    status: String,
    selected: usize,
    session: String,
    next_id: u64,
    keymap: Keymap,
}

impl<C: ShardClient> ShardPlay<C> {
    pub fn connect(client: C, identity: String, keymap: Keymap) -> Self {
        let mut play = Self {
            client,
            identity: identity.clone(),
            view: None,
            json: serde_json::Value::Null,
            disconnected: false,
            status: String::new(),
            selected: 0,
            session: session_nonce(),
            next_id: 1,
            keymap,
        };
        match play.client.join(&identity) {
            Ok(view) => play.show(view),
            Err(err) => play.drop_host(&err.to_string()),
        }
        play
    }

    #[cfg(test)]
    pub fn disconnected(&self) -> bool {
        self.disconnected
    }

    #[cfg(test)]
    pub fn view_json(&self) -> &serde_json::Value {
        &self.json
    }

    #[cfg(test)]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[cfg(test)]
    fn into_client(self) -> C {
        self.client
    }

    pub fn handle_key(&mut self, code: KeyCode) -> ShardAction {
        if self.disconnected {
            return match self
                .keymap
                .lookup(crate::keymap::HelpContext::TitleMenu, code)
            {
                Some(Binding::Quit | Binding::Confirm | Binding::Cancel) => ShardAction::Quit,
                _ => ShardAction::None,
            };
        }
        match self
            .keymap
            .lookup(crate::keymap::HelpContext::TitleMenu, code)
        {
            Some(Binding::Quit) => ShardAction::Quit,
            Some(Binding::SelectNext) => {
                self.selected = (self.selected + 1) % ACTIONS.len();
                ShardAction::None
            }
            Some(Binding::SelectPrev) => {
                self.selected = (self.selected + ACTIONS.len() - 1) % ACTIONS.len();
                ShardAction::None
            }
            Some(Binding::Confirm) => {
                self.submit_selected();
                ShardAction::None
            }
            _ => ShardAction::None,
        }
    }

    fn submit_selected(&mut self) {
        let Some(view) = self.view.clone() else {
            self.drop_host("host is gone");
            return;
        };
        let intent = match ACTIONS[self.selected] {
            "build" => Intent::Build {
                planet: view.home,
                building: BuildingKind::MetalMine,
                actor: Actor::Player,
            },
            "research" => Intent::Research {
                tech: "energy".into(),
            },
            _ => {
                let dest = view
                    .occupancy
                    .iter()
                    .find(|slot| slot.tag == "empty")
                    .map(|slot| slot.coord)
                    .unwrap_or(Coord {
                        galaxy: view.home.galaxy,
                        system: view.home.system,
                        position: view.home.position.saturating_add(1),
                    });
                let mut ships = Fleet::default();
                ships.add(ShipKind::LightFighter, 1);
                Intent::Launch {
                    origin: Some(view.home),
                    ships,
                    dest,
                    speed_percent: 100,
                    hold_secs: 0,
                }
            }
        };
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let cmd = ShardCommand {
            identity: self.identity.clone(),
            idempotency_id: format!("tui-{}-{id}", self.session),
            intent,
            client_now_unix: Some(u64::MAX),
        };
        match self.client.submit(cmd) {
            Ok(view) => {
                self.status = format!("{} sent", ACTIONS[self.selected]);
                self.show(view);
            }
            Err(ClientError::Gone) => self.drop_host("host is gone"),
            Err(ClientError::Refused(msg)) => self.status = msg,
        }
    }

    fn show(&mut self, view: ShardView) {
        self.json = tenant_json(&view);
        self.view = Some(view);
        self.disconnected = false;
    }

    fn drop_host(&mut self, status: &str) {
        self.view = None;
        self.json = serde_json::Value::Null;
        self.disconnected = true;
        self.status = status.into();
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let t = Theme::resolve(Depth::detect(|key| std::env::var(key).ok()), false);
        let area = frame.area();
        crate::ui::chrome::fill(frame.buffer_mut(), area, t.surface(Surface::Base));
        let [nav, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let crumb = if self.disconnected {
            "disconnect"
        } else {
            "shard"
        };
        frame.render_widget(
            Navbar {
                theme: &t,
                mark: "HOSHI",
                crumbs: vec!["Join".into(), crumb.into()],
                right: Vec::new(),
            },
            nav,
        );
        frame.render_widget(
            StatusBar {
                theme: &t,
                crumbs: vec!["join".into(), crumb.into()],
                hints: vec![
                    ("↑↓".into(), "select".into()),
                    (self.keymap.primary(Binding::Confirm), "send".into()),
                    (self.keymap.primary(Binding::Quit), "leave".into()),
                ],
                right: None,
            },
            status,
        );
        let mut lines: Vec<Line> = Vec::new();
        if self.disconnected {
            lines.push(Line::from(Span::styled(
                "host is gone",
                t.intent(ColorIntent::Danger).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::styled("the local save was not opened", t.muted()));
            if !self.status.is_empty() {
                lines.push(Line::from(self.status.clone()));
            }
        } else if let Some(view) = &self.view {
            lines.push(Line::from(Span::styled("shard", t.strong())));
            lines.push(Line::from(format!(
                "identity {}  home {}  at {}",
                view.identity, view.home, view.at
            )));
            lines.push(Line::from(format!(
                "iron {:.0}  silicon {:.0}  deuterium {:.0}",
                view.resources.iron, view.resources.silicon, view.resources.deuterium
            )));
            lines.push(Line::from(""));
            for (i, action) in ACTIONS.iter().enumerate() {
                lines.push(if i == self.selected {
                    Line::from(vec![
                        Span::styled("▌ ", t.intent(ColorIntent::Primary)),
                        Span::styled(format!("{action:<12}"), t.selected()),
                    ])
                } else {
                    Line::styled(format!("  {action}"), t.text())
                });
            }
            if !self.status.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(self.status.clone()));
            }
        }
        frame.render_widget(Paragraph::new(lines), body);
    }
}

fn tenant_json(view: &ShardView) -> serde_json::Value {
    let json = view.to_json();
    debug_assert!(
        !contains_hidden(&json),
        "tenant view leaked a hidden field: {json}"
    );
    json
}

fn contains_hidden(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
            HIDDEN_KEYS
                .iter()
                .any(|hidden| key.eq_ignore_ascii_case(hidden))
                || contains_hidden(child)
        }),
        serde_json::Value::Array(items) => items.iter().any(contains_hidden),
        _ => false,
    }
}

/// In-process host for tests. Production Join never constructs this.
#[cfg(test)]
pub(crate) struct LoopbackHost {
    identity: String,
    shard: ShardSession<FakeClock>,
    empires: BTreeMap<String, GameState>,
    map: ShardMap,
    alive: bool,
    applied: Vec<String>,
}

#[cfg(test)]
fn rules() -> BalanceConfig {
    let mut cfg = BalanceConfig::default();
    cfg.raid.enabled = false;
    cfg.moons.form_chance_per_debris = 0.0;
    cfg
}

#[cfg(test)]
impl LoopbackHost {
    pub(crate) fn new(identity: &str) -> Self {
        let mut host = Self {
            identity: identity.to_string(),
            shard: ShardSession::new(FakeClock::new(1_000), [0x5a; 32], ShardConfig::default()),
            empires: BTreeMap::new(),
            map: ShardMap {
                seed: 7,
                galaxies: 1,
                systems: 4,
                positions: 3,
            },
            alive: true,
            applied: Vec::new(),
        };
        let now = 1_000;
        let rules = rules();
        host.shard
            .join(identity, host.map, &mut host.empires, |_| {
                GameState::with_config(now, rules.clone())
            })
            .expect("join seats the test identity");
        host
    }

    pub(crate) fn kill(&mut self) {
        self.alive = false;
    }

    pub(crate) fn seat_peer(&mut self, identity: &str) {
        let now = 1_000;
        let rules = rules();
        self.shard
            .join(identity, self.map, &mut self.empires, |_| {
                GameState::with_config(now, rules.clone())
            })
            .expect("peer seats");
    }

    pub(crate) fn park(&mut self, kind: ShipKind, n: u32) {
        if let Some(state) = self.empires.get_mut(&self.identity) {
            state.fleet.add(kind, n);
            state.resources.deuterium += 50_000.0;
        }
    }

    pub(crate) fn applied(&self) -> &[String] {
        &self.applied
    }

    fn queued_builds(&self) -> usize {
        self.empires
            .get(&self.identity)
            .map(|state| state.build_queue.len())
            .unwrap_or(0)
    }

    fn require_live(&self) -> Result<(), ClientError> {
        self.alive.then_some(()).ok_or(ClientError::Gone)
    }

    fn view_of(&mut self, identity: &str) -> Result<ShardView, ClientError> {
        self.shard
            .view(identity, &mut self.empires)
            .map_err(map_shard)
    }
}

#[cfg(test)]
impl ShardClient for LoopbackHost {
    fn join(&mut self, identity: &str) -> Result<ShardView, ClientError> {
        self.require_live()?;
        if identity != self.identity {
            self.identity = identity.to_string();
        }
        let now = 1_000;
        let rules = rules();
        self.shard
            .join(identity, self.map, &mut self.empires, |_| {
                GameState::with_config(now, rules.clone())
            })
            .map_err(map_shard)?;
        self.view_of(identity)
    }

    fn submit(&mut self, cmd: ShardCommand) -> Result<ShardView, ClientError> {
        self.require_live()?;
        let kind = match &cmd.intent {
            Intent::Build { .. } => "build",
            Intent::Research { .. } => "research",
            Intent::Launch { .. } => "fleet",
            Intent::Cheat { .. } => {
                return Err(ClientError::Refused("cheat refused".into()));
            }
            _ => "intent",
        };
        let identity = cmd.identity.clone();
        let state = self.empires.get_mut(&identity).ok_or(ClientError::Gone)?;
        self.applied.push(kind.into());
        let result = match self.shard.submit(state, cmd.clone()) {
            Err(ShardError::Unsettled { .. }) => {
                self.shard.settle(&mut self.empires);
                let state = self.empires.get_mut(&identity).ok_or(ClientError::Gone)?;
                self.shard.submit(state, cmd)
            }
            other => other,
        };
        result.map_err(map_shard)?;
        self.view_of(&identity)
    }
}

#[cfg(test)]
fn map_shard(err: ShardError) -> ClientError {
    ClientError::Refused(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_storage::{open_save, save_state};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static UNIQUE: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hoshi-shard-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn render(play: &ShardPlay<LoopbackHost>, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| play.draw(f)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut lines = Vec::new();
        for y in area.y..area.y + area.height {
            let mut line = String::new();
            for x in area.x..area.x + area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    fn write_save(dir: &Path) {
        let mut conn = open_save(&dir.join("save.sqlite")).unwrap();
        save_state(&mut conn, &GameState::new(10)).unwrap();
    }

    #[test]
    fn join_with_a_host_session_shows_the_tenant_view_and_does_not_open_save() {
        let dir = unique_dir();
        write_save(&dir);
        let before = std::fs::metadata(dir.join("save.sqlite")).unwrap();
        let play = ShardPlay::connect(LoopbackHost::new("a"), "a".into(), Keymap::defaults());
        assert!(!play.disconnected());
        let text = render(&play, 80, 24);
        assert!(text.contains("identity a"), "{text}");
        assert!(text.contains("home"), "{text}");
        assert!(!text.contains("rng"), "{text}");
        assert!(!hoshi_storage::save_is_locked(&dir.join("save.sqlite")));
        let after = std::fs::metadata(dir.join("save.sqlite")).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_research_and_fleet_intents_go_to_the_host_not_local_apply() {
        let dir = unique_dir();
        write_save(&dir);
        let local = GameState::new(10);
        let snapshot = local.clone();
        let mut host = LoopbackHost::new("a");
        host.park(ShipKind::LightFighter, 2);
        let mut play = ShardPlay::connect(host, "a".into(), Keymap::defaults());
        play.handle_key(KeyCode::Enter);
        play.handle_key(KeyCode::Char('j'));
        play.handle_key(KeyCode::Enter);
        play.handle_key(KeyCode::Char('j'));
        play.handle_key(KeyCode::Enter);
        assert_eq!(play.client.applied(), ["build", "research", "fleet"]);
        assert_eq!(local, snapshot);
        assert!(!play.disconnected(), "{}", play.status());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn killing_the_host_shows_disconnect_not_the_local_save() {
        let dir = unique_dir();
        write_save(&dir);
        let mut play = ShardPlay::connect(LoopbackHost::new("a"), "a".into(), Keymap::defaults());
        play.client.kill();
        play.handle_key(KeyCode::Enter);
        assert!(play.disconnected());
        let text = render(&play, 80, 24);
        assert!(text.contains("host is gone"), "{text}");
        assert!(text.contains("the local save was not opened"), "{text}");
        assert!(!text.contains("Humans"), "{text}");
        assert!(play.view_json().is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shard_json_omits_hidden_fields_and_the_other_tenant() {
        let mut host = LoopbackHost::new("a");
        host.seat_peer("b");
        let play = ShardPlay::connect(host, "a".into(), Keymap::defaults());
        let dump = play.view_json().to_string();
        assert!(!contains_hidden(play.view_json()), "{dump}");
        assert!(!dump.contains("\"b\""), "{dump}");
        assert!(dump.contains("\"a\""), "{dump}");
        assert!(!dump.contains("rng"), "{dump}");
        assert!(!dump.contains("pirate"), "{dump}");
    }

    #[test]
    fn tick_advance_and_cheats_are_hidden() {
        let play = ShardPlay::connect(LoopbackHost::new("a"), "a".into(), Keymap::defaults());
        let text = render(&play, 80, 24);
        assert!(!text.contains("tick --advance"), "{text}");
        assert!(!text.contains("Cheat console"), "{text}");
        assert!(!text.contains('`'), "{text}");
        let mut host = LoopbackHost::new("a");
        let err = host.submit(ShardCommand {
            identity: "a".into(),
            idempotency_id: "cheat-1".into(),
            intent: Intent::Cheat {
                code: "advance".into(),
                actor: Actor::Player,
            },
            client_now_unix: Some(9_999),
        });
        assert!(matches!(err, Err(ClientError::Refused(_))), "{err:?}");
    }

    #[test]
    fn credentials_live_beside_config_never_in_the_save() {
        let dir = unique_dir();
        write_save(&dir);
        let creds = Credentials {
            identity: "a".into(),
        };
        creds.save(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Credentials::path(&dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let loaded = Credentials::load(&dir).unwrap();
        assert_eq!(loaded.identity, "a");
        Credentials::logout(&dir).unwrap();
        assert!(Credentials::load(&dir).is_none());
        assert!(dir.join("save.sqlite").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconnect_does_not_drop_the_first_intent_as_a_duplicate() {
        let mut play = ShardPlay::connect(LoopbackHost::new("a"), "a".into(), Keymap::defaults());
        play.handle_key(KeyCode::Enter);
        assert_eq!(play.client.queued_builds(), 1);
        let host = play.into_client();
        let mut play = ShardPlay::connect(host, "a".into(), Keymap::defaults());
        play.handle_key(KeyCode::Enter);
        assert_eq!(play.client.queued_builds(), 2);
        assert_eq!(play.client.applied(), ["build", "build"]);
    }

    #[test]
    fn unreachable_host_is_a_disconnect() {
        let play = ShardPlay::connect(UnreachableHost, "a".into(), Keymap::defaults());
        assert!(play.disconnected());
        assert_eq!(play.status(), "host is gone");
    }
}
