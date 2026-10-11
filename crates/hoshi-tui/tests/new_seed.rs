//! `hoshi new --seed` writes a live save from a shareable code.

use hoshi_core::{
    GALAXY_GENERATOR_VERSION, encode_galaxy_seed, load_state, open_save, parse_galaxy_seed,
};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn unique_data() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("hoshi-new-{}-{nanos}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_new(data: &std::path::Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .arg("new")
        .args(extra)
        .env("XDG_DATA_HOME", data)
        .env_remove("HOSHI_SAVE")
        .output()
        .expect("spawn hoshi")
}

#[test]
fn new_seed_writes_a_live_save_from_a_code() {
    let data = unique_data();
    let code = encode_galaxy_seed(42);
    let out = run_new(&data, &["--seed", &code]);
    assert_eq!(out.status.code(), Some(0), "{:?}", out);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains(&code), "{printed}");
    let save = data.join("hoshi").join("save.sqlite");
    let conn = open_save(&save).unwrap();
    let state = load_state(&conn, hoshi_core::BalanceConfig::default()).unwrap();
    assert_eq!(state.galaxy_seed, 42);
    assert_eq!(state.generator_version, GALAXY_GENERATOR_VERSION);
    assert!(!state.deterministic);
    let again = run_new(&data, &["--seed", "42"]);
    assert_eq!(again.status.code(), Some(2), "{:?}", again);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn new_seed_honors_hoshi_save() {
    let data = unique_data();
    let save = data.join("custom.sqlite");
    let out = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .args(["new", "--seed", "42"])
        .env("XDG_DATA_HOME", &data)
        .env("HOSHI_SAVE", &save)
        .output()
        .expect("spawn hoshi");
    assert_eq!(out.status.code(), Some(0), "{:?}", out);
    assert!(save.exists());
    assert!(!data.join("hoshi").join("save.sqlite").exists());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn new_seed_rejects_a_bad_code() {
    let data = unique_data();
    let out = run_new(&data, &["--seed", "nope"]);
    assert_eq!(out.status.code(), Some(2), "{:?}", out);
    assert!(parse_galaxy_seed("nope").is_err());
    let _ = std::fs::remove_dir_all(&data);
}
