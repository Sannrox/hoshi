use hoshi_core::{BalanceConfig, GameState};
use hoshi_storage::{open_save, save_state};
use std::io::Read;
use std::process::Command;

#[test]
fn local_report_archive_redacts_and_save_inclusion_is_explicit() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hoshi-report-cli-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let save = dir.join("save.sqlite");
    let mut conn = open_save(&save).unwrap();
    let mut state = GameState::with_seed(100, BalanceConfig::default(), 987654321);
    state.record_history(hoshi_core::HistoryDraft {
        at: 100,
        action: "build_complete".into(),
        outcome: "solar plant 3".into(),
        ..Default::default()
    });
    save_state(&mut conn, &state).unwrap();
    std::fs::write(dir.join("config.toml"), "future_secret = \"\"\"\nmultiline-credential\n\"\"\"\nserve_oidc = true\nserve_token = \"config-credential\"\nserve_vapid_private = \"vapid-credential\"\nserve_oidc_issuer = \"https://private.example.test\"\n").unwrap();
    std::fs::create_dir(dir.join("logs")).unwrap();
    std::fs::write(
        dir.join("logs/hoshi.log"),
        "Bearer env-credential\nprivate.example.test 10.2.3.4\nlocalhost 127.0.0.1\n",
    )
    .unwrap();
    for include in [false, true] {
        let out = dir.join(if include {
            "with-save.tar.gz"
        } else {
            "without-save.tar.gz"
        });
        let mut command = Command::new(env!("CARGO_BIN_EXE_hoshi"));
        command
            .args([
                "--save",
                save.to_str().unwrap(),
                "report",
                "--yes",
                "--format",
                "archive",
                "--out",
                out.to_str().unwrap(),
            ])
            .env("HOSHI_SERVE_TOKEN", "env-credential")
            .env("HOSHI_SERVE_OIDC_TOKEN", "oidc-credential")
            .env("HOME", &dir)
            .env("USERPROFILE", &dir)
            .env("USER", "report-fixture-user");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            command.env(
                "UNRELATED_NONUNICODE",
                std::ffi::OsString::from_vec(vec![0xff]),
            );
        }
        if include {
            command.arg("--include-save");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let decoder = flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap());
        let mut tar = tar::Archive::new(decoder);
        let mut has_save = false;
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if name == "save.sqlite" {
                has_save = true;
                let snapshot = dir.join("attached.sqlite");
                std::fs::write(&snapshot, bytes).unwrap();
                let read = open_save(&snapshot).unwrap();
                let loaded = hoshi_storage::load_state(&read, BalanceConfig::default()).unwrap();
                assert_eq!(loaded.galaxy_seed, state.galaxy_seed);
                continue;
            }
            let text = String::from_utf8(bytes).unwrap();
            for secret in [
                "multiline-credential",
                "config-credential",
                "vapid-credential",
                "env-credential",
                "oidc-credential",
                "private.example.test",
                "10.2.3.4",
                dir.to_str().unwrap(),
            ] {
                assert!(!text.contains(secret), "{name} leaked {secret}: {text}");
            }
            if name == "report.md" {
                assert!(text.contains("Bundled default: true"), "{text}");
                assert!(!text.contains("987654321"), "seed leaked: {text}");
                for field in [
                    "Version:",
                    "Commit:",
                    "CLI contract:",
                    "Save schema",
                    "Terminal:",
                    "SHA-256",
                    "solar plant 3",
                    "Balance",
                    "Environment",
                ] {
                    assert!(text.contains(field), "missing {field}: {text}");
                }
                assert_eq!(
                    text.contains("WARNING: The archive includes the save"),
                    include
                );
                assert!(text.contains("localhost") && text.contains("127.0.0.1"));
            }
        }
        assert_eq!(has_save, include);
        assert!(String::from_utf8_lossy(&output.stdout).contains("report"));
    }
    drop(conn);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn damaged_save_still_produces_metadata_report_but_cannot_be_attached() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hoshi-report-damaged-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let save = dir.join("save.sqlite");
    std::fs::write(&save, b"damaged save").unwrap();
    let out = dir.join("report");
    let output = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .args([
            "--save",
            save.to_str().unwrap(),
            "report",
            "--yes",
            "--out",
            "report",
        ])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = std::fs::read_to_string(&out).unwrap();
    assert!(report.contains("Summary unavailable") && report.contains("SHA-256"));
    let archive = dir.join("report.tar.gz");
    let output = Command::new(env!("CARGO_BIN_EXE_hoshi"))
        .args([
            "--save",
            save.to_str().unwrap(),
            "report",
            "--yes",
            "--include-save",
            "--out",
            archive.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!archive.exists());
    assert_eq!(std::fs::read(&save).unwrap(), b"damaged save");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collection_only_contacts_explicit_loopback_health_endpoint() {
    use std::io::Write;
    use std::net::TcpListener;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hoshi-report-network-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let save = dir.join("missing.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    std::fs::write(
        dir.join("config.toml"),
        format!("serve_oidc_issuer = \"http://{address}/never\"\n"),
    )
    .unwrap();
    let run = |name: &str| {
        Command::new(env!("CARGO_BIN_EXE_hoshi"))
            .args([
                "--save",
                save.to_str().unwrap(),
                "report",
                "--yes",
                "--out",
                dir.join(name).to_str().unwrap(),
            ])
            .output()
            .unwrap()
    };
    assert!(run("offline.md").status.success());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::write(
        dir.join("config.toml"),
        format!("serve_bind = \"{address}\"\n"),
    )
    .unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(error) => panic!("health probe missing: {error}"),
            }
        };
        // The accepted stream can inherit the listener's non-blocking mode,
        // which turns a request still in flight into an immediate WouldBlock.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 1024];
        let count = stream
            .read(&mut request)
            .unwrap_or_else(|error| panic!("health probe sent no request: {error}"));
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(request.starts_with("GET /healthz HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("authorization"));
        let body = "{\"ok\":true,\"contract_versions\":[1],\"version\":\"0.0.0\"}";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let output = run("local.md");
    worker.join().unwrap();
    assert!(output.status.success());
    let report = std::fs::read_to_string(dir.join("local.md")).unwrap();
    assert!(
        report.contains("Configured loopback health:") && report.contains("Version mismatch: true")
    );
    std::fs::remove_dir_all(dir).unwrap();
}
