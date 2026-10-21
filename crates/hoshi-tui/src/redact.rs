//! The persistence boundary for diagnostic text. No caller writes raw diagnostics.
use regex::Regex;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

pub struct Redactor {
    secrets: Vec<String>,
    home: String,
    username: String,
    rules: Vec<(&'static str, Regex)>,
}

pub struct Redacted {
    pub text: String,
    pub counts: BTreeMap<&'static str, usize>,
}

impl Redactor {
    pub fn for_dir(dir: &Path) -> Self {
        let config = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
        let mut secrets = Vec::new();
        for line in config.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if sensitive_key(key.trim()) && key.trim() != "serve_oidc" {
                let raw = value.trim();
                let value = serde_json::from_str::<String>(raw).unwrap_or_else(|_| {
                    raw.split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(['\"', '\''])
                        .to_owned()
                });
                if key.trim() == "serve_token_file" {
                    let path = Path::new(&value);
                    let path = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        dir.join(path)
                    };
                    if let Ok(tokens) = std::fs::read_to_string(path) {
                        for line in tokens
                            .lines()
                            .filter(|line| !line.trim_start().starts_with('#'))
                        {
                            if let Some(token) = line.split_whitespace().next() {
                                secrets.push(token.to_owned());
                            }
                        }
                    }
                }
                secrets.push(value);
            }
        }
        if let Ok(config) = toml::from_str::<toml::Value>(&config) {
            collect_secrets(&config, false, &mut secrets);
        }
        for (key, value) in std::env::vars_os() {
            let (Ok(key), Ok(value)) = (key.into_string(), value.into_string()) else {
                continue;
            };
            if (key.starts_with("HOSHI_SERVE_") && key != "HOSHI_SERVE_OIDC" && sensitive_key(&key))
                || matches!(key.as_str(), "HOSTNAME" | "COMPUTERNAME")
            {
                secrets.push(value);
            }
        }
        if let Some(path) = std::env::var_os("HOSHI_SERVE_TOKEN_FILE")
            && let Ok(tokens) = std::fs::read_to_string(path)
        {
            secrets.extend(
                tokens
                    .lines()
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .filter_map(|line| line.split_whitespace().next())
                    .map(str::to_owned),
            );
        }
        Self::new(
            secrets,
            std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default(),
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_default(),
        )
    }

    fn new(mut secrets: Vec<String>, home: String, username: String) -> Self {
        secrets.retain(|value| !value.is_empty());
        secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
        secrets.dedup();
        let rules = [
            ("secret_field", r#"(?im)^\s*[^=\n]*(?:token|password|secret|vapid_private|serve_oidc)[^=\n]*=\s*[^\n]*"#),
            ("bearer", r#"(?i)\bbearer\s+[^\s\"']+"#),
            ("url", r#"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>]+"#),
            ("address", r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b"),
            ("address", r"(?:[0-9a-fA-F]{0,4}:){2,}[0-9a-fA-F:.]*"),
            ("hostname", r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*\b"),
            ("seed", r"(?i)\b(?:galaxy_seed|rng_seed|seed_code|galaxy seed|seed code)\s*[:=]\s*[^\s,;]+"),
        ].into_iter().map(|(name, pattern)| (name, Regex::new(pattern).expect("static redaction rule"))).collect();
        Self {
            secrets,
            home,
            username,
            rules,
        }
    }

    pub fn redact_config(&self, input: &str) -> Redacted {
        if input.trim().is_empty() {
            return self.redact(input);
        }
        let Ok(mut config) = toml::from_str::<toml::Value>(input) else {
            return self.redact("Config unavailable: invalid TOML; values excluded.");
        };
        let count = redact_values(&mut config);
        let text = toml::to_string(&config)
            .unwrap_or_else(|_| "Config unavailable; values excluded.".into());
        let mut redacted = self.redact(&text);
        if count > 0 {
            *redacted.counts.entry("secret_field").or_default() += count;
        }
        redacted
    }

    pub fn redact(&self, input: &str) -> Redacted {
        let mut text = input.to_owned();
        let mut counts = BTreeMap::new();
        for secret in &self.secrets {
            let count = text.matches(secret).count();
            if count > 0 {
                text = text.replace(secret, "<redacted>");
                *counts.entry("known_secret").or_default() += count;
            }
        }
        for (needle, replacement, rule) in [
            (&self.home, "~", "home"),
            (&self.username, "<user>", "username"),
        ] {
            if !needle.is_empty() {
                let count = text.matches(needle).count();
                if count > 0 {
                    text = text.replace(needle, replacement);
                    *counts.entry(rule).or_default() += count;
                }
            }
        }
        for (name, rule) in &self.rules {
            let mut changed = 0;
            text = rule
                .replace_all(&text, |captures: &regex::Captures<'_>| {
                    let raw = &captures[0];
                    let replacement = match *name {
                        "address" => match raw.parse::<IpAddr>() {
                            Ok(address) if !address.is_loopback() => "<address>".to_owned(),
                            _ => raw.to_owned(),
                        },
                        "url" => {
                            let authority = raw
                                .split_once("://")
                                .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
                                .unwrap_or("");
                            let safe = (authority == "localhost"
                                || authority.starts_with("localhost:")
                                || authority.starts_with("127.0.0.1:")
                                || authority == "127.0.0.1"
                                || authority.starts_with("[::1]"))
                                && !authority.contains('@');
                            if safe {
                                raw.split(['?', '#']).next().unwrap_or(raw).to_owned()
                            } else {
                                "<url>".into()
                            }
                        }
                        "secret_field" => format!(
                            "{}= <redacted>",
                            raw.split_once('=').map(|(key, _)| key).unwrap_or("secret")
                        ),
                        "bearer" => "Bearer <redacted>".into(),
                        "hostname" => "<hostname>".into(),
                        "seed" => "galaxy_seed: <redacted>".into(),
                        _ => "<redacted>".into(),
                    };
                    if replacement != raw {
                        changed += 1;
                    }
                    replacement
                })
                .into_owned();
            if changed > 0 {
                *counts.entry(name).or_default() += changed;
            }
        }
        Redacted { text, counts }
    }
}

fn collect_secrets(value: &toml::Value, sensitive: bool, secrets: &mut Vec<String>) {
    match value {
        toml::Value::String(text) if sensitive => {
            secrets.push(text.clone());
            secrets.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            );
        }
        toml::Value::Table(table) => {
            for (key, value) in table {
                collect_secrets(
                    value,
                    sensitive || (sensitive_key(key) && key != "serve_oidc"),
                    secrets,
                );
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                collect_secrets(value, sensitive, secrets);
            }
        }
        _ => {}
    }
}

fn redact_values(value: &mut toml::Value) -> usize {
    match value {
        toml::Value::Table(table) => table
            .iter_mut()
            .map(|(key, value)| {
                if sensitive_key(key) && key != "serve_oidc" {
                    *value = toml::Value::String("<redacted>".into());
                    1
                } else {
                    redact_values(value)
                }
            })
            .sum(),
        toml::Value::Array(values) => values.iter_mut().map(redact_values).sum(),
        _ => 0,
    }
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["token", "secret", "password", "vapid_private", "serve_oidc"]
        .iter()
        .any(|part| key.contains(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_remove_identity_credentials_and_private_addresses() {
        let redactor = Redactor::new(
            vec!["opaque-key".into()],
            "/home/example".into(),
            "example-user".into(),
        );
        let output = redactor.redact("serve_token = \"opaque-key\"\nAuthorization: Bearer other-key\n/home/example/log example-user 10.2.3.4 ::ffff:192.168.1.4 private.example.net https://user:pass@private.example.net/a?token=secret\nlocalhost 127.0.0.1 ::1 http://localhost:123/a");
        for secret in [
            "opaque-key",
            "other-key",
            "/home/example",
            "example-user",
            "10.2.3.4",
            "192.168.1.4",
            "private.example.net",
            "user:pass",
            "token=secret",
        ] {
            assert!(!output.text.contains(secret), "{secret}: {}", output.text);
        }
        for safe in ["localhost", "127.0.0.1", "::1", "http://localhost:123/a"] {
            assert!(output.text.contains(safe), "{safe}: {}", output.text);
        }
        assert!(!output.counts.is_empty());
    }
}
