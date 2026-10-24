//! Help is a read-only front door even when the selected save is locked.
use std::process::Command;

#[test]
fn human_help_never_opens_the_save() {
    let path = std::env::temp_dir().join(format!(
        "hoshi-help-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let lock = hoshi_storage::lock_save(&path).unwrap();
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["help", "state"],
        vec!["state", "--help"],
        vec!["serve", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hoshi"))
            .arg("--save")
            .arg(&path)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Usage: hoshi"), "{text}");
        assert!(!text.contains("\"schema_version\""), "{text}");
        assert!(!path.exists(), "help created a save");
    }
    drop(lock);
    let _ = std::fs::remove_file(hoshi_storage::save_lock_path(&path));
}

#[cfg(unix)]
#[test]
fn bash_completes_commands_flags_and_nested_help() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/generated/hoshi.bash");
    for (words, expected) in [
        (vec!["hoshi", ""], "state"),
        (vec!["hoshi", "--"], "--save"),
        (vec!["hoshi", "--save", "game.sqlite", "fleet", ""], "send"),
        (
            vec!["hoshi", "--save", "=", "game.sqlite", "fleet", ""],
            "send",
        ),
        (
            vec!["hoshi", "--balance", "=", "balance.toml", "fleet", ""],
            "send",
        ),
        (vec!["hoshi", "--seed", "=", "1", "fleet", ""], "send"),
        (vec!["hoshi", "help", "fleet", ""], "send"),
        (vec!["hoshi", "help", "market", "contract", ""], "accept"),
    ] {
        let output = Command::new("bash")
            .args(["-c", "source \"$1\"; shift; COMP_WORDS=(\"$@\"); COMP_CWORD=$((${#COMP_WORDS[@]}-1)); _hoshi; printf '%s\\n' \"${COMPREPLY[@]}\"", "_"])
            .arg(&script).args(&words).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let candidates = String::from_utf8(output.stdout).unwrap();
        assert!(
            candidates.lines().any(|word| word == expected),
            "{words:?}: {candidates}"
        );
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash").args(["-c", "source \"$1\"; COMP_WORDS=(hoshi --save \"$2\"); COMP_CWORD=2; _hoshi; printf '%s\\n' \"${COMPREPLY[@]}\"", "_"]).arg(&script).arg(manifest.join("Cargo.t")).output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|candidate| candidate == manifest.join("Cargo.toml").to_str().unwrap())
    );
    let output = Command::new("bash").args(["-c", "source \"$1\"; COMP_WORDS=(hoshi --save = \"$2\"); COMP_CWORD=3; _hoshi; printf '%s\\n' \"${COMPREPLY[@]}\"", "_"]).arg(&script).arg(manifest.join("Cargo.t")).output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|candidate| candidate == manifest.join("Cargo.toml").to_str().unwrap())
    );
    let output = Command::new("bash").args(["-c", "source \"$1\"; COMP_WORDS=(hoshi automation add \"\"); COMP_CWORD=3; _hoshi; printf '%s\\n' \"${COMPREPLY[@]}\"", "_"]).arg(&script).output().unwrap();
    assert!(output.status.success());
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|word| word == "transfer")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn zsh_completes_on_first_autoload() {
    let dir = std::env::temp_dir().join(format!(
        "hoshi-help-zsh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/generated/hoshi.zsh"),
        dir.join("_hoshi"),
    )
    .unwrap();
    let output = Command::new("zsh").args(["-fc", "zmodload zsh/parameter; fpath=(\"$1\" $fpath); compadd() { print -r -- \"$@\"; }; autoload -Uz _hoshi; words=(hoshi help fleet \"\"); CURRENT=4; _hoshi", "_"]).arg(&dir).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidates = String::from_utf8(output.stdout).unwrap();
    assert!(
        candidates.split_whitespace().any(|word| word == "send"),
        "{candidates}"
    );
}
