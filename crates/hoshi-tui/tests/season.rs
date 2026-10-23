//! `hoshi --save PATH season` accepts the global flag before the command.

use std::process::Command;

#[test]
fn season_lists_after_a_leading_save_flag() {
    let save = std::env::temp_dir().join(format!(
        "hoshi-season-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .arg("--save")
        .arg(&save)
        .arg("season")
        .output()
        .expect("spawn hoshi");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.starts_with("window "), "{stdout}");
}
