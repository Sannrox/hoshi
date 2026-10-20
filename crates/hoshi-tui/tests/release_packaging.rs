//! Release-note and Homebrew formula helpers used by `.github/workflows/release.yml`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

#[test]
fn release_tag_must_match_workspace_version() {
    let root = repo_root();
    let version = env!("CARGO_PKG_VERSION");
    let ok = Command::new("bash")
        .arg(root.join("scripts/check-release-tag.sh"))
        .arg(format!("v{version}"))
        .current_dir(&root)
        .output()
        .expect("check-release-tag.sh");
    assert!(
        ok.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let stdout = String::from_utf8_lossy(&ok.stdout);
    assert_eq!(stdout.trim(), version);
    let bad = Command::new("bash")
        .arg(root.join("scripts/check-release-tag.sh"))
        .arg("v9.9.9")
        .current_dir(&root)
        .output()
        .expect("check-release-tag.sh mismatch");
    assert!(!bad.status.success());
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("v9.9.9"), "{stderr}");
    assert!(
        stderr.contains(&format!("crate version {version}")),
        "{stderr}"
    );
}

#[test]
fn release_notes_include_changelog_and_save_schema() {
    let root = repo_root();
    let output = Command::new("bash")
        .arg(root.join("scripts/release-notes.sh"))
        .arg("v0.1.0")
        .current_dir(&root)
        .output()
        .expect("release-notes.sh");
    assert!(
        output.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# hoshi 0.1.0"), "{stdout}");
    assert!(stdout.contains("## Saves"), "{stdout}");
    assert!(stdout.contains("save schema v"), "{stdout}");
    assert!(
        stdout.contains("Opening an older save migrates it in place"),
        "{stdout}"
    );
}

#[test]
fn release_notes_select_bracketed_version_heading() {
    let root = repo_root();
    let dist = tempfile_dist();
    let changelog = dist.path().join("CHANGELOG.md");
    fs::write(
        &changelog,
        "# Changelog\n\n## Unreleased\n\n- wrong section\n\n## [0.1.0] - 2026-10-16\n\n- tagged notes\n",
    )
    .unwrap();
    let output = Command::new("bash")
        .arg(root.join("scripts/release-notes.sh"))
        .arg("v0.1.0")
        .env("CHANGELOG_PATH", &changelog)
        .current_dir(&root)
        .output()
        .expect("release-notes.sh versioned heading");
    assert!(
        output.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("- tagged notes"), "{stdout}");
    assert!(!stdout.contains("wrong section"), "{stdout}");
    assert!(stdout.contains("save schema v"), "{stdout}");
}

#[test]
fn homebrew_formula_hashes_raw_binaries() {
    let root = repo_root();
    let dist = tempfile_dist();
    for name in [
        "hoshi-aarch64-apple-darwin",
        "hoshi-x86_64-apple-darwin",
        "hoshi-aarch64-unknown-linux-gnu",
        "hoshi-x86_64-unknown-linux-gnu",
        "hoshi-x86_64-pc-windows-msvc.exe",
    ] {
        fs::write(dist.path().join(name), name.as_bytes()).unwrap();
    }
    let output = Command::new("bash")
        .arg(root.join("scripts/homebrew-formula.sh"))
        .arg("v0.1.0")
        .arg(dist.path())
        .current_dir(&root)
        .output()
        .expect("homebrew-formula.sh");
    assert!(
        output.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let formula = fs::read_to_string(dist.path().join("hoshi.rb")).unwrap();
    assert!(formula.contains("class Hoshi < Formula"), "{formula}");
    assert!(formula.contains("version \"0.1.0\""), "{formula}");
    assert!(formula.contains("using: :nounzip"), "{formula}");
    assert!(
        formula.contains("assert_match version.to_s, shell_output(\"#{bin}/hoshi --version\")"),
        "{formula}"
    );
    assert!(
        formula.contains("https://github.com/Sannrox/hoshi/releases/download/v0.1.0/"),
        "{formula}"
    );
}

#[test]
fn build_deb_rejects_versions_outside_release_and_nightly() {
    let root = repo_root();
    let dist = tempfile_dist();
    let bin = dist.path().join("hoshi");
    fs::write(&bin, b"stub").unwrap();
    for version in [
        "0.1.0",
        "v0.1",
        "v0.1.0-rc1",
        "v0.1.0~nightly",
        "v0.1.0~nightly.1.XYZ",
    ] {
        let output = Command::new("bash")
            .arg(root.join("scripts/build-deb.sh"))
            .arg(version)
            .arg("amd64")
            .arg(&bin)
            .arg(dist.path())
            .output()
            .expect("build-deb.sh");
        assert_eq!(output.status.code(), Some(2), "{version} accepted");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("usage:"), "{version}: {stderr}");
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tempfile_dist() -> TempDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "hoshi-formula-{}-{nanos}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    TempDir(path)
}

#[cfg(target_os = "linux")]
#[test]
fn deb_includes_man_and_shell_completions() {
    let root = repo_root();
    let dist = tempfile_dist();
    let output = Command::new("bash")
        .arg(root.join("scripts/build-deb.sh"))
        .args(["v0.1.0", "amd64", env!("CARGO_BIN_EXE_hoshi")])
        .arg(dist.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new("dpkg-deb")
        .arg("--contents")
        .arg(dist.path().join("hoshi_0.1.0_amd64.deb"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let contents = String::from_utf8(output.stdout).unwrap();
    for path in [
        "usr/share/man/man1/hoshi.1",
        "usr/share/bash-completion/completions/hoshi",
        "usr/share/zsh/vendor-completions/_hoshi",
        "usr/share/fish/vendor_completions.d/hoshi.fish",
    ] {
        assert!(contents.contains(path), "missing {path}: {contents}");
    }
}
