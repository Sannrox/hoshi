//! Launch `hoshi serve` and probe `/healthz`.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: &[u8],
    authorization: Option<&str>,
) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let auth_line = match authorization {
        Some(value) => format!("Authorization: {value}\r\n"),
        None => String::new(),
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth_line}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    let status = buf
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let text = buf
        .split("\r\n\r\n")
        .nth(1)
        .or_else(|| buf.split("\n\n").nth(1))
        .unwrap_or("")
        .to_string();
    Ok((status, text))
}

fn get_path(addr: SocketAddr, path: &str) -> std::io::Result<(u16, String)> {
    request(addr, "GET", path, b"", None)
}

const TEST_TOKEN: &str = "bin-test-token";

fn unique_save() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "hoshi-serve-bin-{}-{nanos}.sqlite",
        std::process::id()
    ))
}

fn spawn_serve(save: &std::path::Path) -> (std::process::Child, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .args([
            "serve",
            "--save",
            save.to_str().unwrap(),
            "--bind",
            "127.0.0.1:0",
        ])
        .env("HOSHI_SERVE_TOKEN", TEST_TOKEN)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hoshi serve");
    let stdout = child.stdout.take().expect("serve stdout");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("serve banner");
    let value: Value = serde_json::from_str(line.trim()).unwrap_or_else(|_| {
        panic!("serve banner JSON: {line}");
    });
    assert_eq!(value["ok"], true, "{line}");
    let bind = value["bind"].as_str().expect("bind");
    let addr: SocketAddr = bind.parse().expect("socket");
    (child, addr)
}

#[test]
fn binary_healthz_twice_and_lock_blocks_cli() {
    let save = unique_save();
    let (mut child, addr) = spawn_serve(&save);
    assert!(addr.ip().is_loopback());

    let (status1, body1) = get_path(addr, "/healthz").expect("healthz 1");
    let (status2, body2) = get_path(addr, "/healthz").expect("healthz 2");
    assert_eq!(status1, 200, "{body1}");
    assert_eq!(status2, 200, "{body2}");
    let v: Value = serde_json::from_str(body1.trim()).unwrap();
    assert_eq!(v["ok"], true);
    assert!(v["contract_versions"].as_array().is_some());
    assert!(
        v["contract_versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str() == Some("v1")),
        "{v}"
    );

    let bearer = Some("Bearer bin-test-token");
    let (denied, denied_body) = request(addr, "GET", "/v1/state", b"", None).expect("unauth");
    assert_eq!(denied, 401, "{denied_body}");
    let denied_json: Value = serde_json::from_str(denied_body.trim()).unwrap();
    assert_eq!(denied_json["error"]["code"], "unauthorized");

    let (s1, b1) = request(addr, "GET", "/v1/state", b"", bearer).expect("state 1");
    let (s2, b2) = request(addr, "GET", "/v1/state", b"", bearer).expect("state 2");
    assert_eq!(s1, 200, "{b1}");
    assert_eq!(s2, 200, "{b2}");
    let state: Value = serde_json::from_str(b1.trim()).unwrap();
    assert_eq!(state["ok"], true);
    assert_eq!(state["schema_version"], 2);
    let (build_status, build_body) = request(
        addr,
        "POST",
        "/v1/build",
        br#"{"planet":"home","building":"metal_mine"}"#,
        bearer,
    )
    .expect("build");
    assert_eq!(build_status, 200, "{build_body}");
    let built: Value = serde_json::from_str(build_body.trim()).unwrap();
    assert_eq!(built["ok"], true, "{built}");
    assert_eq!(built["message"], "queued build");

    let cli = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .args(["--save", save.to_str().unwrap(), "--seed", "1", "state"])
        .output()
        .expect("cli state");
    assert_eq!(cli.status.code(), Some(4), "{:?}", cli);

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&save);
    let _ = std::fs::remove_file(format!("{}.lock", save.display()));
}
