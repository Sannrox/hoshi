use std::net::TcpListener;
use std::process::{Command, Stdio};

#[test]
fn cli_requires_consent_and_offline_reporting_never_contacts_the_target() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hoshi-bug-cli-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let target = format!(
        "https://{}/new?title={{title}}&body={{body}}",
        listener.local_addr().unwrap()
    );
    std::fs::write(
        dir.join("config.toml"),
        format!("bug_report_target = {target:?}\nserve_token = \"test-private-token\"\n"),
    )
    .unwrap();
    let save = dir.join("save.sqlite");
    let run = |options: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hoshi"))
            .args([
                "--save",
                save.to_str().unwrap(),
                "bug",
                "--title",
                "test-private-token failure",
                "--happened",
                "Stopped",
                "--expected",
                "Continue",
            ])
            .args(options)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let denied = run(&["--offline"]);
    assert!(!denied.status.success());
    assert!(!dir.join("reports").exists());
    let offline = run(&["--offline", "--yes"]);
    assert!(
        offline.status.success(),
        "{}",
        String::from_utf8_lossy(&offline.stderr)
    );
    let files: Vec<_> = std::fs::read_dir(dir.join("reports")).unwrap().collect();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
    assert!(text.contains("## Issue page"));
    assert!(!text.contains("test-private-token"));
    assert!(!String::from_utf8_lossy(&offline.stdout).contains("test-private-token"));
    let printed = run(&["--print-url", "--yes"]);
    assert!(printed.status.success());
    assert!(
        String::from_utf8_lossy(&printed.stdout)
            .contains(&target.split('?').next().unwrap().to_owned())
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::remove_dir_all(dir).unwrap();
}
