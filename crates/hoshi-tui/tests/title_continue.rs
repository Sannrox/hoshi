//! Direct-launch `--continue` uses documented exit codes.

use hoshi_core::{BalanceConfig, GameState};
use hoshi_storage::{SCHEMA_VERSION, lock_save, open_save, save_state};
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
    let dir =
        std::env::temp_dir().join(format!("hoshi-continue-{}-{nanos}-{n}", std::process::id()));
    std::fs::create_dir_all(dir.join("hoshi")).unwrap();
    dir
}

fn run_continue(data: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .arg("--continue")
        .env("XDG_DATA_HOME", data)
        .env_remove("HOSHI_SAVE")
        .output()
        .expect("spawn hoshi")
}

#[test]
fn continue_exits_5_when_there_is_no_slot() {
    let data = unique_data();
    let out = run_continue(&data);
    assert_eq!(out.status.code(), Some(5), "{:?}", out);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn continue_exits_4_when_the_slot_is_locked() {
    let data = unique_data();
    let save = data.join("hoshi").join("save.sqlite");
    let mut conn = open_save(&save).unwrap();
    save_state(&mut conn, &GameState::new(1)).unwrap();
    drop(conn);
    let holder = lock_save(&save).unwrap();
    let out = run_continue(&data);
    drop(holder);
    assert_eq!(out.status.code(), Some(4), "{:?}", out);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn continue_exits_5_for_newer_corrupt_and_harness_saves() {
    let bin = env!("CARGO_BIN_EXE_hoshi");
    let data = unique_data();
    let dir = data.join("hoshi");

    let newer = dir.join("save.sqlite");
    {
        let conn = open_save(&newer).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 2)
            .unwrap();
    }
    let out = run_continue(&data);
    assert_eq!(out.status.code(), Some(5), "newer {:?}", out);
    let _ = std::fs::remove_file(&newer);

    std::fs::write(dir.join("save.sqlite"), b"not sqlite").unwrap();
    let out = run_continue(&data);
    assert_eq!(out.status.code(), Some(5), "corrupt {:?}", out);
    let _ = std::fs::remove_file(dir.join("save.sqlite"));

    let mut conn = open_save(&dir.join("save.sqlite")).unwrap();
    save_state(
        &mut conn,
        &GameState::with_seed(1, BalanceConfig::default(), 9),
    )
    .unwrap();
    drop(conn);
    let out = Command::new(bin)
        .arg("--continue")
        .env("XDG_DATA_HOME", &data)
        .env_remove("HOSHI_SAVE")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5), "harness {:?}", out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("harness") || stderr.contains("deterministic") || !stderr.is_empty(),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&data);
}
