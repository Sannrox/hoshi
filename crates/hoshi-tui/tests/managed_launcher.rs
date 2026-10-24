#![cfg(unix)]
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
#[test]
fn published_launcher_is_executable_and_follows_each_atomic_pointer() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("hoshi-launcher-{}-{nonce}", std::process::id()));
    // Install the launcher once, like the updater's atomic rename. Copying
    // over an executable that has already run rewrites it in place, and
    // macOS may kill the next exec of that file for a stale code signature.
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("update.lock"), "").unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_hoshi"), root.join("bin/hoshi")).unwrap();
    for version in ["0.1.0", "0.2.0"] {
        let release = root.join("releases").join(version);
        std::fs::create_dir_all(&release).unwrap();
        let binary = release.join("hoshi");
        std::fs::write(&binary, format!("#!/bin/sh\nprintf '{version}\\n'\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("current"), format!("{version}\n\n")).unwrap();
        let path = root.join("bin/hoshi");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let output = Command::new(path).output().unwrap();
        assert!(
            output.status.success(),
            "launching {version} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), version);
    }
    let binary = root.join("releases/0.2.0/hoshi");
    std::fs::write(
        &binary,
        br#"#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
IFS= read -r input
printf '%s' "$input" > "$root/input"
printf '%s' "$$" > "$root/child-pid"
exec sleep 60
"#,
    )
    .unwrap();
    let mut child = Command::new(root.join("bin/hoshi"))
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"terminal input\n")
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !root.join("child-pid").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(root.join("child-pid")).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("input")).unwrap(),
        "terminal input"
    );
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let reaped = child.try_wait().unwrap().is_some();
    let alive = Command::new("kill")
        .args(["-0", &pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    if alive {
        let _ = Command::new("kill").args(["-KILL", &pid]).status();
    }
    if !reaped {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        reaped && !alive,
        "launcher must forward TERM and reap its child"
    );
    std::fs::remove_dir_all(root).unwrap();
}
