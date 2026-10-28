//! Opt-in web push for raid warnings and finished builds.
//!
//! Operator VAPID keys live in runtime config. Browser subscriptions live in a
//! file next to the save (`<save>.push.json`). Neither enters the save.

use crate::notify::{self, Watch};
use crate::webpush::{Receiver, Vapid};
use hoshi_core::GameState;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

/// Subscriptions kept per save; more are refused until one is removed.
pub const MAX_SUBSCRIPTIONS: usize = 16;

/// Player opt-in plus operator VAPID material. Empty keys keep the channel off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub opt_in: bool,
    pub vapid_private: String,
    pub vapid_subject: String,
}

impl Settings {
    pub fn from_config(cfg: &crate::config::UserConfig) -> Self {
        Self {
            opt_in: env_flag("HOSHI_SERVE_PUSH").unwrap_or(cfg.serve_push),
            vapid_private: env_nonempty("HOSHI_SERVE_VAPID_PRIVATE")
                .unwrap_or_else(|| cfg.serve_vapid_private.clone()),
            vapid_subject: env_nonempty("HOSHI_SERVE_VAPID_SUBJECT")
                .unwrap_or_else(|| cfg.serve_vapid_subject.clone()),
        }
    }

    /// The operator key when push is on; `None` keeps the channel off.
    fn vapid(&self) -> Option<Vapid> {
        if !self.opt_in || self.vapid_subject.is_empty() {
            return None;
        }
        Vapid::from_private(&self.vapid_private)
    }
}

fn env_flag(key: &str) -> Option<bool> {
    let value = std::env::var(key).ok()?;
    match value.as_str() {
        "1" | "true" | "TRUE" | "yes" => Some(true),
        "0" | "false" | "FALSE" | "no" => Some(false),
        _ => None,
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    (!value.is_empty()).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Build,
    Raid,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Raid => "raid",
        }
    }

    /// Client view to open, relative to the web client root.
    fn url(self) -> &'static str {
        match self {
            Self::Build => "#/colony",
            Self::Raid => "#/bridge",
        }
    }
}

/// Public notice body. Hidden simulation stays off this object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

impl Payload {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "title": self.title,
            "body": self.body,
            "url": self.kind.url(),
        })
    }
}

/// A browser `PushSubscription` as the client posts it (`toJSON()` shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: Keys,
    /// The device session that registered it, set by the server from the
    /// request's credential (never the body). Revoking the device drops it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

impl Subscription {
    /// Only browser push services over HTTPS, with keys that encrypt.
    pub fn valid(&self) -> bool {
        self.endpoint.len() <= 2048
            && !self
                .endpoint
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '"')
            && push_service(&self.endpoint)
            && Receiver::parse(&self.keys.p256dh, &self.keys.auth).is_some()
    }
}

/// Hosts the major browsers hand out as push endpoints. The server only ever
/// POSTs to these, so a subscription cannot point it at an internal address.
const PUSH_HOSTS: &[&str] = &["fcm.googleapis.com", "web.push.apple.com"];
const PUSH_HOST_SUFFIXES: &[&str] = &[
    ".push.services.mozilla.com",
    ".notify.windows.com",
    ".push.apple.com",
];

/// `https://<allowed host>/…` with no userinfo and no port other than 443.
fn push_service(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "443")) => host,
        Some(_) => return false,
        None => authority,
    }
    .to_ascii_lowercase();
    PUSH_HOSTS.contains(&host.as_str())
        || PUSH_HOST_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The push service answered 404 or 410: the subscription is gone.
    Gone,
    Failed,
}

pub trait Transport: Send + Sync {
    fn send(&self, to: &Subscription, payload: &serde_json::Value) -> Delivery;
}

/// Encrypts each payload for its subscriber and POSTs it with a VAPID header.
pub struct Https {
    vapid: Vapid,
    subject: String,
}

impl Https {
    pub fn new(settings: &Settings) -> Option<Self> {
        Some(Self {
            vapid: settings.vapid()?,
            subject: settings.vapid_subject.clone(),
        })
    }

    /// Headers and encrypted body for one message; `None` if it cannot be sealed.
    fn request(
        &self,
        to: &Subscription,
        payload: &serde_json::Value,
        now: u64,
    ) -> Option<(String, Vec<u8>)> {
        let receiver = Receiver::parse(&to.keys.p256dh, &to.keys.auth)?;
        let body = crate::webpush::seal(&receiver, payload.to_string().as_bytes())?;
        let authorization = self.vapid.authorization(&to.endpoint, &self.subject, now)?;
        Some((authorization, body))
    }
}

impl Transport for Https {
    fn send(&self, to: &Subscription, payload: &serde_json::Value) -> Delivery {
        use hoshi_core::Clock;
        let now = hoshi_core::SystemClock.now_unix();
        let Some((authorization, body)) = self.request(to, payload, now) else {
            return Delivery::Failed;
        };
        match post(&to.endpoint, &authorization, &body) {
            Ok(status) => delivery(status),
            Err(_) => Delivery::Failed,
        }
    }
}

fn delivery(status: u16) -> Delivery {
    match status {
        200..=299 => Delivery::Sent,
        404 | 410 => Delivery::Gone,
        _ => Delivery::Failed,
    }
}

/// One bounded POST through `curl`, like the update check. The VAPID header
/// goes through a private config file, not argv, so `ps` never shows it.
fn post(endpoint: &str, authorization: &str, body: &[u8]) -> std::io::Result<u16> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let config = std::env::temp_dir().join(format!(
        "hoshi-push-{}-{nanos}-{n}.curl",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&config)
        .and_then(|mut file| {
            write!(
                file,
                "url = \"{endpoint}\"\nheader = \"Authorization: {authorization}\"\n"
            )
        })
        .and_then(|()| run_curl(&config, body));
    let _ = std::fs::remove_file(&config);
    result
}

fn run_curl(config: &Path, body: &[u8]) -> std::io::Result<u16> {
    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--globoff",
            "--silent",
            "--max-time",
            "10",
            "--connect-timeout",
            "10",
            "--proto",
            "=https",
            "--request",
            "POST",
            "--header",
            "TTL: 86400",
            "--header",
            "Content-Encoding: aes128gcm",
            "--header",
            "Content-Type: application/octet-stream",
            "--data-binary",
            "@-",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            "--config",
        ])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body)?;
    }
    let mut code = String::new();
    if let Some(stdout) = child.stdout.take() {
        stdout.take(16).read_to_string(&mut code)?;
    }
    child.wait()?;
    code.trim()
        .parse()
        .map_err(|_| std::io::Error::other("no HTTP status"))
}

/// Subscriptions file for a save: `<save file>.push.json` beside it.
pub fn store_path(save: &Path) -> PathBuf {
    let mut name = save.file_name().unwrap_or_default().to_os_string();
    name.push(".push.json");
    save.with_file_name(name)
}

/// Stored subscriptions. A missing file is empty; an unreadable or malformed
/// one is an error, so a later write never replaces subscriptions it lost.
fn load(path: &Path) -> std::io::Result<Vec<Subscription>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let subs: Vec<Subscription> = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    Ok(subs.into_iter().filter(Subscription::valid).collect())
}

fn store(path: &Path, subs: &[Subscription]) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(subs).map_err(std::io::Error::other)?;
    atomicwrites::AtomicFile::new(path, atomicwrites::OverwriteBehavior::AllowOverwrite)
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

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Off,
    Invalid,
    Full,
    Storage,
}

/// Messages taken from the gate, delivered without holding its lock.
pub struct Outbox {
    transport: Arc<dyn Transport>,
    messages: Vec<(Subscription, serde_json::Value)>,
}

impl Outbox {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Send everything; returns endpoints the push service reported gone.
    /// Up to [`MAX_SUBSCRIPTIONS`] sends run at once, so a stalled push
    /// service costs one request timeout, not one per subscriber.
    pub fn deliver(self) -> Vec<String> {
        let transport = self.transport.as_ref();
        let mut gone = Vec::new();
        for batch in self.messages.chunks(MAX_SUBSCRIPTIONS) {
            let results: Vec<(String, Delivery)> = std::thread::scope(|scope| {
                let sends: Vec<_> = batch
                    .iter()
                    .map(|(to, payload)| {
                        scope.spawn(move || (to.endpoint.clone(), transport.send(to, payload)))
                    })
                    .collect();
                sends
                    .into_iter()
                    .filter_map(|send| send.join().ok())
                    .collect()
            });
            for (endpoint, delivery) in results {
                match delivery {
                    Delivery::Sent => {}
                    Delivery::Gone => gone.push(endpoint),
                    Delivery::Failed => {
                        crate::diagnostics::log("warn", "web push delivery failed");
                    }
                }
            }
        }
        gone.sort();
        gone.dedup();
        gone
    }
}

/// Watches the game for notices and fans them out to stored subscriptions.
pub struct Gate {
    public_key: Option<String>,
    watch: Watch,
    transport: Arc<dyn Transport>,
    path: PathBuf,
    pending: Vec<serde_json::Value>,
}

impl Gate {
    /// Off unless opt-in, subject and a valid VAPID private key are all set.
    pub fn new(
        settings: &Settings,
        state: &GameState,
        save: &Path,
        transport: Option<Arc<dyn Transport>>,
    ) -> Self {
        let vapid = settings.vapid();
        let public_key = vapid.as_ref().map(Vapid::public_key);
        let transport = transport
            .or_else(|| Https::new(settings).map(|https| Arc::new(https) as Arc<dyn Transport>))
            .unwrap_or_else(|| Arc::new(Off));
        Self {
            public_key,
            watch: Watch::from_state(state),
            transport,
            path: store_path(save),
            pending: Vec::new(),
        }
    }

    pub fn armed(&self) -> bool {
        self.public_key.is_some()
    }

    /// VAPID public key for `GET /v1/push/key`; `None` while push is off.
    pub fn public_key(&self) -> Option<&str> {
        self.public_key.as_deref()
    }

    /// Store or replace (by endpoint) a browser subscription.
    pub fn subscribe(&mut self, sub: Subscription) -> Result<(), Refused> {
        if !self.armed() {
            return Err(Refused::Off);
        }
        if !sub.valid() {
            return Err(Refused::Invalid);
        }
        let mut subs = load(&self.path).map_err(|_| Refused::Storage)?;
        subs.retain(|old| old.endpoint != sub.endpoint);
        if subs.len() >= MAX_SUBSCRIPTIONS {
            return Err(Refused::Full);
        }
        subs.push(sub);
        store(&self.path, &subs).map_err(|_| Refused::Storage)
    }

    /// Remove a subscription by endpoint; `Ok(false)` when it was not stored.
    pub fn unsubscribe(&mut self, endpoint: &str) -> Result<bool, Refused> {
        let mut subs = load(&self.path).map_err(|_| Refused::Storage)?;
        let before = subs.len();
        subs.retain(|sub| sub.endpoint != endpoint);
        if subs.len() == before {
            return Ok(false);
        }
        store(&self.path, &subs).map_err(|_| Refused::Storage)?;
        Ok(true)
    }

    /// Queue build and raid notices; [`Gate::take_outbox`] sends them.
    pub fn poll(&mut self, state: &GameState, now: u64) {
        let notices = self.watch.observe(state, now);
        if !self.armed() {
            return;
        }
        for notice in notices {
            let kind = match notice.kind {
                notify::Kind::Build => Kind::Build,
                notify::Kind::Raid => Kind::Raid,
                notify::Kind::Fleet | notify::Kind::Summary => continue,
            };
            let payload = Payload {
                kind,
                title: notice.title.clone(),
                body: notice.body.clone(),
            };
            self.pending.push(payload.json());
        }
    }

    /// Pair queued payloads with every stored subscription whose device
    /// session is still `live` (subscriptions without a device always are).
    /// Subscriptions of revoked or expired devices are dropped from the file,
    /// so a failed cleanup is retried on the next notice.
    pub fn take_outbox(&mut self, live: impl Fn(&str) -> bool) -> Outbox {
        let payloads = std::mem::take(&mut self.pending);
        let mut subs = if payloads.is_empty() {
            Vec::new()
        } else {
            load(&self.path).unwrap_or_else(|_| {
                crate::diagnostics::log(
                    "warn",
                    "web push subscriptions unreadable; notice dropped",
                );
                Vec::new()
            })
        };
        let before = subs.len();
        subs.retain(|sub| sub.device.as_deref().is_none_or(&live));
        if subs.len() != before && store(&self.path, &subs).is_err() {
            crate::diagnostics::log("warn", "revoked devices' push subscriptions not removed");
        }
        let messages = payloads
            .iter()
            .flat_map(|payload| subs.iter().map(|sub| (sub.clone(), payload.clone())))
            .collect();
        Outbox {
            transport: Arc::clone(&self.transport),
            messages,
        }
    }
}

/// Transport while push is off; nothing is queued, so it never runs.
struct Off;

impl Transport for Off {
    fn send(&self, _to: &Subscription, _payload: &serde_json::Value) -> Delivery {
        Delivery::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{BuildingKind, Fleet, GameState, Mission, MissionPhase, ShipKind};
    use std::sync::Mutex;

    // RFC 8291 example keys: the server key and one browser subscription.
    const VAPID_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const UA_AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

    /// Records sends and answers each endpoint with a fixed delivery.
    struct Recording {
        sent: Mutex<Vec<(String, serde_json::Value)>>,
        answer: Delivery,
    }

    impl Recording {
        fn new(answer: Delivery) -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(Vec::new()),
                answer,
            })
        }

        fn sent(&self) -> Vec<(String, serde_json::Value)> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl Transport for Recording {
        fn send(&self, to: &Subscription, payload: &serde_json::Value) -> Delivery {
            self.sent
                .lock()
                .unwrap()
                .push((to.endpoint.clone(), payload.clone()));
            self.answer
        }
    }

    fn armed() -> Settings {
        Settings {
            opt_in: true,
            vapid_private: VAPID_PRIVATE.into(),
            vapid_subject: "mailto:hoshi@localhost".into(),
        }
    }

    fn unique_save() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "hoshi-push-test-{}-{nanos}-{n}.sqlite",
            std::process::id()
        ))
    }

    fn gate_with(
        settings: &Settings,
        state: &GameState,
        answer: Delivery,
    ) -> (Gate, Arc<Recording>, PathBuf) {
        let save = unique_save();
        let recording = Recording::new(answer);
        let transport: Arc<dyn Transport> = recording.clone();
        let gate = Gate::new(settings, state, &save, Some(transport));
        (gate, recording, store_path(&save))
    }

    fn armed_gate(state: &GameState) -> (Gate, Arc<Recording>, PathBuf) {
        gate_with(&armed(), state, Delivery::Sent)
    }

    fn browser(endpoint: &str) -> Subscription {
        Subscription {
            endpoint: endpoint.into(),
            keys: Keys {
                p256dh: UA_PUBLIC.into(),
                auth: UA_AUTH.into(),
            },
            device: None,
        }
    }

    fn subscribe(gate: &mut Gate, endpoint: &str) {
        gate.subscribe(browser(endpoint)).expect("subscribe");
    }

    #[test]
    fn finished_build_and_raid_warning_reach_every_subscription() {
        let mut state = GameState::new(1_000);
        let (mut gate, recording, path) = armed_gate(&state);
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/a");
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/b");
        state.buildings.increment(BuildingKind::MetalMine);
        gate.poll(&state, 1_000);
        state.next_raid_at = Some(1_050);
        gate.poll(&state, 1_000);
        assert!(gate.take_outbox(|_| true).deliver().is_empty());
        let sent = recording.sent();
        assert_eq!(sent.len(), 4, "{sent:?}");
        for endpoint in ["a", "b"] {
            for kind in ["build", "raid"] {
                let to = format!("https://fcm.googleapis.com/fcm/send/{endpoint}");
                assert!(
                    sent.iter().any(|(at, p)| *at == to && p["kind"] == kind),
                    "{kind} to {to} missing: {sent:?}"
                );
            }
        }
        let build = &sent.iter().find(|(_, p)| p["kind"] == "build").unwrap().1;
        assert_eq!(build["body"], "build finished");
        assert_eq!(build["url"], "#/colony");
        let raid = &sent.iter().find(|(_, p)| p["kind"] == "raid").unwrap().1;
        assert_eq!(raid["url"], "#/bridge");
        assert!(
            raid["body"]
                .as_str()
                .unwrap_or("")
                .contains("raid incoming")
        );
        assert!(!raid.to_string().contains("next_raid_at"), "{raid}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn revoked_device_stops_receiving_notices() {
        let mut state = GameState::new(1_000);
        let (mut gate, recording, path) = armed_gate(&state);
        let lost = "https://fcm.googleapis.com/fcm/send/lost";
        let kept = "https://fcm.googleapis.com/fcm/send/kept";
        gate.subscribe(Subscription {
            device: Some("lost-phone".into()),
            ..browser(lost)
        })
        .unwrap();
        subscribe(&mut gate, kept);
        state.buildings.increment(BuildingKind::MetalMine);
        gate.poll(&state, 1_000);
        gate.take_outbox(|device| device != "lost-phone").deliver();
        let sent: Vec<String> = recording.sent().into_iter().map(|(to, _)| to).collect();
        assert_eq!(sent, vec![kept.to_string()]);
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(lost), "{stored}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unsubscribe_stops_delivery_and_gone_endpoints_are_dropped() {
        let mut state = GameState::new(1_000);
        let (mut gate, recording, path) = gate_with(&armed(), &state, Delivery::Gone);
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/a");
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/b");
        assert_eq!(
            gate.unsubscribe("https://fcm.googleapis.com/fcm/send/a"),
            Ok(true)
        );
        assert_eq!(
            gate.unsubscribe("https://fcm.googleapis.com/fcm/send/a"),
            Ok(false)
        );
        state.buildings.increment(BuildingKind::MetalMine);
        gate.poll(&state, 1_000);
        let gone = gate.take_outbox(|_| true).deliver();
        assert_eq!(gone, ["https://fcm.googleapis.com/fcm/send/b"]);
        assert_eq!(recording.sent().len(), 1);
        for endpoint in gone {
            assert_eq!(gate.unsubscribe(&endpoint), Ok(true));
        }
        assert!(load(&path).unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn subscribe_refuses_when_off_invalid_or_full() {
        let state = GameState::new(1_000);
        let mut off = armed();
        off.opt_in = false;
        let (mut gate, _, path) = gate_with(&off, &state, Delivery::Sent);
        assert!(gate.public_key().is_none());
        assert_eq!(
            gate.subscribe(browser("https://fcm.googleapis.com/fcm/send/a")),
            Err(Refused::Off)
        );
        assert!(!path.exists());

        let (mut gate, _, path) = armed_gate(&state);
        assert_eq!(
            gate.public_key(),
            Some(
                "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
            )
        );
        assert_eq!(
            gate.subscribe(browser("http://fcm.googleapis.com/fcm/send/a")),
            Err(Refused::Invalid)
        );
        assert_eq!(
            gate.subscribe(browser("https://fcm.googleapis.com/fcm/send/a b")),
            Err(Refused::Invalid)
        );
        let mut bad_keys = browser("https://fcm.googleapis.com/fcm/send/a");
        bad_keys.keys.auth = "short".into();
        assert_eq!(gate.subscribe(bad_keys), Err(Refused::Invalid));
        for n in 0..MAX_SUBSCRIPTIONS {
            subscribe(
                &mut gate,
                &format!("https://fcm.googleapis.com/fcm/send/{n}"),
            );
        }
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/0");
        assert_eq!(
            gate.subscribe(browser("https://fcm.googleapis.com/fcm/send/extra")),
            Err(Refused::Full)
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_push_when_opt_in_off_or_vapid_incomplete() {
        let mut state = GameState::new(1_000);
        let mut no_opt_in = armed();
        no_opt_in.opt_in = false;
        let mut no_key = armed();
        no_key.vapid_private = "k".into();
        let mut no_subject = armed();
        no_subject.vapid_subject.clear();
        for settings in [no_opt_in, no_key, no_subject] {
            let (mut gate, recording, path) = gate_with(&settings, &state, Delivery::Sent);
            assert!(!gate.armed());
            state.buildings.increment(BuildingKind::MetalMine);
            state.next_raid_at = Some(1_050);
            gate.poll(&state, 1_000);
            gate.take_outbox(|_| true).deliver();
            assert!(recording.sent().is_empty());
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn https_request_decrypts_with_the_subscriber_keys() {
        let https = Https::new(&armed()).expect("armed");
        let payload =
            serde_json::json!({"kind": "raid", "title": "hoshi", "body": "raid incoming"});
        let (authorization, body) = https
            .request(
                &browser("https://fcm.googleapis.com/fcm/send/a"),
                &payload,
                1_000,
            )
            .expect("request");
        assert!(authorization.starts_with("vapid t="), "{authorization}");
        let ua = p256::SecretKey::from_slice(&crate::b64url::decode(UA_PRIVATE).unwrap()).unwrap();
        let auth: [u8; 16] = crate::b64url::decode(UA_AUTH).unwrap().try_into().unwrap();
        let plain = crate::webpush::open(&ua, &auth, &body).expect("decrypts");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&plain).unwrap(),
            payload
        );
        assert_eq!(delivery(201), Delivery::Sent);
        assert_eq!(delivery(410), Delivery::Gone);
        assert_eq!(delivery(404), Delivery::Gone);
        assert_eq!(delivery(429), Delivery::Failed);
    }

    #[test]
    fn only_browser_push_services_are_accepted() {
        for ok in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
            "https://web.push.apple.com/abc",
            "https://FCM.googleapis.com:443/fcm/send/abc",
        ] {
            assert!(push_service(ok), "{ok}");
        }
        for bad in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/push",
            "https://localhost/push",
            "https://internal.corp/push",
            "https://fcm.googleapis.com:8443/fcm/send/abc",
            "https://fcm.googleapis.com@10.0.0.1/x",
            "https://evilfcm.googleapis.com.attacker.net/x",
            "https://notify.windows.com.attacker.net/x",
        ] {
            assert!(!push_service(bad), "{bad}");
        }
    }

    #[test]
    fn a_corrupt_subscription_file_is_never_overwritten() {
        let state = GameState::new(1_000);
        let (mut gate, _, path) = armed_gate(&state);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(
            gate.subscribe(browser("https://fcm.googleapis.com/fcm/send/a")),
            Err(Refused::Storage)
        );
        assert_eq!(
            gate.unsubscribe("https://fcm.googleapis.com/fcm/send/a"),
            Err(Refused::Storage)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn subscriptions_live_beside_the_save() {
        let save = Path::new("/srv/hoshi/home.sqlite");
        assert_eq!(
            store_path(save),
            Path::new("/srv/hoshi/home.sqlite.push.json")
        );
    }

    #[test]
    fn fleet_return_does_not_push() {
        let mut state = GameState::new(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 1);
        state.missions.push(Mission {
            id: 1,
            ships,
            origin: hoshi_core::Coord::HOME,
            dest: hoshi_core::Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            speed_percent: 100,
            phase: MissionPhase::Return,
            launched_at: 1_000,
            outbound_secs: 10,
            hold_secs: 0,
            phase_ends_at: 1_100,
            fuel_paid: 0.0,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: hoshi_core::Resources::ZERO,
            transport: false,
            attack: false,
            invade: false,
            spy: false,
            site_id: None,
        });
        let (mut gate, recording, path) = armed_gate(&state);
        subscribe(&mut gate, "https://fcm.googleapis.com/fcm/send/a");
        state.missions.clear();
        gate.poll(&state, 1_100);
        gate.take_outbox(|_| true).deliver();
        assert!(recording.sent().is_empty());
        let _ = std::fs::remove_file(path);
    }
}
