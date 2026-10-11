//! Two processes cannot write the same save.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn second_cli_gets_locked_while_the_lock_is_held() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let save =
        std::env::temp_dir().join(format!("hoshi-lock-{}-{nanos}.sqlite", std::process::id()));
    let bin = env!("CARGO_BIN_EXE_hoshi");
    let holder = hoshi_core::lock_save(&save).expect("holder lock");
    let output = Command::new(bin)
        .args(["--save", save.to_str().unwrap(), "--seed", "1", "state"])
        .output()
        .expect("spawn cli");
    drop(holder);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(4), "{stdout}");
    assert!(stdout.contains("\"locked\""), "{stdout}");
    let _ = std::fs::remove_file(&save);
    let _ = std::fs::remove_file(format!("{}.lock", save.display()));
}
