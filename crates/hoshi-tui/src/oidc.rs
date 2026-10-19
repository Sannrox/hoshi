//! Optional Authorization Code with PKCE for `hoshi serve`.
//!
//! Operator identity-provider settings stay in runtime config, never the save.
//! A successful login mints a save-scoped bearer token. Bearer remains the default.

use sha2::{Digest, Sha256};

/// Opt-in plus identity-provider endpoints. Incomplete values keep the flow off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub opt_in: bool,
    pub issuer: String,
    pub client_id: String,
    pub authorization: String,
    pub token: String,
}

impl Settings {
    pub fn from_config(cfg: &crate::config::UserConfig) -> Self {
        Self {
            opt_in: env_flag("HOSHI_SERVE_OIDC").unwrap_or(cfg.serve_oidc),
            issuer: env_nonempty("HOSHI_SERVE_OIDC_ISSUER")
                .unwrap_or_else(|| cfg.serve_oidc_issuer.clone()),
            client_id: env_nonempty("HOSHI_SERVE_OIDC_CLIENT_ID")
                .unwrap_or_else(|| cfg.serve_oidc_client_id.clone()),
            authorization: env_nonempty("HOSHI_SERVE_OIDC_AUTHORIZATION")
                .unwrap_or_else(|| cfg.serve_oidc_authorization.clone()),
            token: env_nonempty("HOSHI_SERVE_OIDC_TOKEN")
                .unwrap_or_else(|| cfg.serve_oidc_token.clone()),
        }
    }

    pub fn armed(&self) -> bool {
        self.opt_in
            && !self.issuer.is_empty()
            && !self.client_id.is_empty()
            && !self.authorization.is_empty()
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

/// Token-endpoint request the relying party sends after the callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRequest {
    pub code: String,
    pub verifier: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub nonce: String,
}

/// Subject returned by a successful exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub sub: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeError {
    Unavailable,
}

pub trait TokenExchange: Send {
    fn exchange(&mut self, req: &TokenRequest) -> Result<Identity, ExchangeError>;
}

/// Production stand-in until a live token POST exists.
pub struct Noop;

impl TokenExchange for Noop {
    fn exchange(&mut self, _req: &TokenRequest) -> Result<Identity, ExchangeError> {
        Err(ExchangeError::Unavailable)
    }
}

struct Pending {
    state: String,
    nonce: String,
    verifier: String,
}

pub struct Gate {
    settings: Settings,
    pending: Option<Pending>,
    transport: Box<dyn TokenExchange>,
}

impl Gate {
    pub fn new(settings: Settings, transport: Box<dyn TokenExchange>) -> Self {
        Self {
            settings,
            pending: None,
            transport,
        }
    }

    pub fn noop(settings: Settings) -> Self {
        Self::new(settings, Box::new(Noop))
    }

    pub fn armed(&self) -> bool {
        self.settings.armed()
    }

    /// Build the authorization redirect. `entropy` is 64 bytes of CSPRNG output.
    pub fn start(&mut self, redirect_uri: &str, entropy: [u8; 64]) -> Result<String, StartError> {
        if !self.settings.armed() {
            return Err(StartError::Off);
        }
        let verifier = b64url(&entropy[..32]);
        let state = b64url(&entropy[32..48]);
        let nonce = b64url(&entropy[48..64]);
        let challenge = s256(&verifier);
        self.pending = Some(Pending {
            state: state.clone(),
            nonce: nonce.clone(),
            verifier,
        });
        Ok(authorize_url(
            &self.settings.authorization,
            &self.settings.client_id,
            redirect_uri,
            &state,
            &nonce,
            &challenge,
        ))
    }

    pub fn callback(
        &mut self,
        code: &str,
        state: &str,
        redirect_uri: &str,
    ) -> Result<Identity, CallbackError> {
        if !self.settings.armed() {
            return Err(CallbackError::Off);
        }
        if code.is_empty() {
            return Err(CallbackError::Invalid);
        }
        let pending = self.pending.as_ref().ok_or(CallbackError::NoPending)?;
        if pending.state != state {
            return Err(CallbackError::State);
        }
        let pending = self.pending.take().ok_or(CallbackError::NoPending)?;
        let req = TokenRequest {
            code: code.to_string(),
            verifier: pending.verifier,
            redirect_uri: redirect_uri.to_string(),
            client_id: self.settings.client_id.clone(),
            nonce: pending.nonce,
        };
        self.transport
            .exchange(&req)
            .map_err(|_| CallbackError::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackError {
    Off,
    Invalid,
    NoPending,
    State,
    Unavailable,
}

/// 32-byte secret as a URL-safe bearer token.
pub fn secret_from_entropy(entropy: [u8; 32]) -> String {
    b64url(&entropy)
}

pub fn query_value(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

pub fn entropy() -> std::io::Result<[u8; 64]> {
    read_entropy()
}

pub fn secret_entropy() -> std::io::Result<[u8; 32]> {
    let mut buf = [0u8; 32];
    fill_random(&mut buf)?;
    Ok(buf)
}

fn read_entropy() -> std::io::Result<[u8; 64]> {
    let mut buf = [0u8; 64];
    fill_random(&mut buf)?;
    Ok(buf)
}

fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    getrandom::getrandom(buf).map_err(|err| std::io::Error::other(err.to_string()))
}

pub(crate) fn s256(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

fn authorize_url(
    authorization: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let sep = if authorization.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{authorization}{sep}response_type=code&client_id={}&redirect_uri={}&scope=openid&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
        percent_encode(client_id),
        percent_encode(redirect_uri),
        percent_encode(state),
        percent_encode(nonce),
        percent_encode(challenge),
    )
}

fn b64url(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i];
        let b1 = data.get(i + 1).copied();
        let b2 = data.get(i + 2).copied();
        out.push(T[(b0 >> 2) as usize] as char);
        match (b1, b2) {
            (None, _) => {
                out.push(T[((b0 & 3) << 4) as usize] as char);
            }
            (Some(b1), None) => {
                out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
                out.push(T[((b1 & 15) << 2) as usize] as char);
            }
            (Some(b1), Some(b2)) => {
                out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
                out.push(T[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char);
                out.push(T[(b2 & 63) as usize] as char);
            }
        }
        i += 3;
    }
    out
}

fn percent_encode(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = from_hex(bytes[i + 1]);
            let lo = from_hex(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
pub struct Recording {
    pub last: std::sync::Mutex<Option<TokenRequest>>,
}

#[cfg(test)]
impl Default for Recording {
    fn default() -> Self {
        Self {
            last: std::sync::Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl TokenExchange for std::sync::Arc<Recording> {
    fn exchange(&mut self, req: &TokenRequest) -> Result<Identity, ExchangeError> {
        *self.last.lock().expect("recording") = Some(req.clone());
        Ok(Identity {
            sub: "player".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn armed() -> Settings {
        Settings {
            opt_in: true,
            issuer: "http://127.0.0.1/issuer".into(),
            client_id: "hoshi".into(),
            authorization: "http://127.0.0.1/authorize".into(),
            token: "http://127.0.0.1/token".into(),
        }
    }

    #[test]
    fn off_does_not_start() {
        let mut gate = Gate::noop(Settings::default());
        assert!(!gate.armed());
        assert_eq!(
            gate.start("http://127.0.0.1/v1/oidc/callback", [1u8; 64]),
            Err(StartError::Off)
        );
    }

    #[test]
    fn pkce_flow_issues_identity_with_matching_verifier() {
        let rec = Arc::new(Recording::default());
        let mut gate = Gate::new(armed(), Box::new(Arc::clone(&rec)));
        let redirect = "http://127.0.0.1:9/v1/oidc/callback";
        let location = gate.start(redirect, [7u8; 64]).expect("start");
        assert!(location.contains("response_type=code"));
        assert!(location.contains("code_challenge_method=S256"));
        assert!(location.contains("client_id=hoshi"));
        assert!(location.contains("scope=openid"));
        let challenge = query_value(
            &format!("x?{}", location.split_once('?').unwrap().1),
            "code_challenge",
        )
        .expect("challenge");
        let state = query_value(
            &format!("x?{}", location.split_once('?').unwrap().1),
            "state",
        )
        .expect("state");
        let identity = gate
            .callback("one-time-code", &state, redirect)
            .expect("callback");
        assert_eq!(identity.sub, "player");
        let req = rec.last.lock().expect("last").clone().expect("exchanged");
        assert_eq!(req.code, "one-time-code");
        assert_eq!(s256(&req.verifier), challenge);
        assert_eq!(req.redirect_uri, redirect);
        assert_eq!(req.client_id, "hoshi");
    }

    #[test]
    fn callback_rejects_wrong_state() {
        let rec = Arc::new(Recording::default());
        let mut gate = Gate::new(armed(), Box::new(rec));
        let redirect = "http://127.0.0.1/v1/oidc/callback";
        let location = gate.start(redirect, [3u8; 64]).unwrap();
        let query = location.split_once('?').map(|(_, q)| q).unwrap_or("");
        let state = query_value(&format!("x?{query}"), "state").expect("state");
        assert_eq!(
            gate.callback("code", "nope", redirect),
            Err(CallbackError::State)
        );
        assert!(gate.callback("code", &state, redirect).is_ok());
    }

    #[test]
    fn incomplete_settings_are_unarmed() {
        let mut s = armed();
        s.opt_in = false;
        assert!(!s.armed());
        s.opt_in = true;
        s.client_id.clear();
        assert!(!s.armed());
    }
}
