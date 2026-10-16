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
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("hoshi-formula-{}-{}", std::process::id(), nanos));
    fs::create_dir_all(&path).unwrap();
    TempDir(path)
}
