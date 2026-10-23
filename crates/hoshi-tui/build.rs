use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(feature = "ui")]
#[path = "src/ui_pin.rs"]
mod ui_pin;

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

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // Without `ui` the table is empty and nothing is downloaded.
    #[cfg(feature = "ui")]
    let files = {
        println!("cargo:rerun-if-changed=hoshi-web.pin");
        let pin = ui_pin::Pin::parse(
            &std::fs::read_to_string("hoshi-web.pin").expect("read hoshi-web.pin"),
        )
        .unwrap_or_else(|err| panic!("hoshi-web.pin: {err}"));
        let zip = fetch(&pin, &out);
        unpack(&zip, &out.join("hoshi-web"))
    };
    #[cfg(not(feature = "ui"))]
    let files = Vec::new();
    write_table(&out.join("ui_files.rs"), &files);
}

/// The pinned zip, downloaded once into `OUT_DIR` and checked on every build.
#[cfg(feature = "ui")]
fn fetch(pin: &ui_pin::Pin, out: &Path) -> Vec<u8> {
    let path = out.join(format!("hoshi-web-{}.zip", pin.tag));
    if let Ok(bytes) = std::fs::read(&path)
        && pin.verify(&bytes).is_ok()
    {
        return bytes;
    }
    let partial = path.with_extension("zip.part");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--max-time",
            "300",
            "--output",
        ])
        .arg(&partial)
        .arg(pin.url())
        .status()
        .unwrap_or_else(|err| panic!("run curl for {}: {err}", pin.url()));
    assert!(status.success(), "download {} failed: {status}", pin.url());
    let bytes = std::fs::read(&partial).expect("read downloaded hoshi-web zip");
    if let Err(err) = pin.verify(&bytes) {
        let _ = std::fs::remove_file(&partial);
        panic!("{err}");
    }
    std::fs::rename(&partial, &path).expect("keep hoshi-web zip");
    bytes
}

/// Extract regular files under `dir`; returns their bundle-relative paths.
#[cfg(feature = "ui")]
fn unpack(zip: &[u8], dir: &Path) -> Vec<(String, PathBuf)> {
    let _ = std::fs::remove_dir_all(dir);
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip)).expect("hoshi-web zip is readable");
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("hoshi-web zip entry");
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .enclosed_name()
            .unwrap_or_else(|| panic!("hoshi-web zip entry {index} escapes the bundle"));
        let rel = name
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let target = dir.join(&name);
        std::fs::create_dir_all(target.parent().expect("entry parent")).expect("create dir");
        let mut file = std::fs::File::create(&target).expect("create bundle file");
        std::io::copy(&mut entry, &mut file).expect("extract bundle file");
        files.push((rel, target));
    }
    assert!(
        files.iter().any(|(rel, _)| rel == "index.html"),
        "hoshi-web zip has no index.html"
    );
    files.sort();
    files
}

fn write_table(path: &Path, files: &[(String, PathBuf)]) {
    let mut code = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (rel, file) in files {
        code.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", file));
    }
    code.push_str("];\n");
    std::fs::write(path, code).expect("write ui_files.rs");
}
