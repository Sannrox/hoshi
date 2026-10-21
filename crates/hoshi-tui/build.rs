use std::process::Command;
fn main() {
    println!("cargo:rerun-if-env-changed=HOSHI_COMMIT");
    for name in ["HEAD".to_owned(), "packed-refs".to_owned()]
        .into_iter()
        .chain(
            Command::new("git")
                .args(["symbolic-ref", "-q", "HEAD"])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|name| name.trim().to_owned()),
        )
    {
        if let Ok(out) = Command::new("git")
            .args(["rev-parse", "--git-path", &name])
            .output()
            && out.status.success()
        {
            println!(
                "cargo:rerun-if-changed={}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
    }
    let commit = std::env::var("HOSHI_COMMIT")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=HOSHI_COMMIT={}", commit.trim());
}
