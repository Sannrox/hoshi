//! `hoshi --version` / `-V` print the package version and exit.

use std::process::Command;

#[test]
fn version_flag_prints_the_package_version() {
    let bin = env!("CARGO_BIN_EXE_hoshi");
    for flag in ["--version", "-V"] {
        let output = Command::new(bin).arg(flag).output().expect("spawn hoshi");
        assert!(
            output.status.success(),
            "{flag} status {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("hoshi "), "{flag} stdout {stdout:?}");
        assert!(
            stdout.contains(env!("CARGO_PKG_VERSION")),
            "{flag} stdout {stdout:?}"
        );
        assert!(
            output.stderr.is_empty(),
            "{flag} leaked stderr {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
