//! Prepare locally, preview, then publish only after the caller obtains consent.
use crate::{redact::Redactor, support};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PUBLIC_TARGET: &str = "https://github.com/Sannrox/hoshi/issues/new";
const URL_LIMIT: usize = 6_000;
const TRUNCATED: &str = "…truncated, see attached report";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fields {
    pub title: String,
    pub happened: String,
    pub expected: String,
    pub steps: String,
}

pub struct Prepared {
    pub report: support::Report,
    pub url: String,
    pub search_url: Option<String>,
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    use std::fmt::Write;
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn shorten(text: &str, count: usize) -> String {
    if text.chars().count() <= count {
        text.into()
    } else {
        let prefix: String = text.chars().take(count).collect();
        format!("{prefix}\n{TRUNCATED}")
    }
}

pub fn issue_url(target: &str, title: &str, body: &str) -> Result<String, String> {
    build_url(target, title, body, "")
}

fn build_url(target: &str, title: &str, body: &str, extra: &str) -> Result<String, String> {
    if target == "off" {
        return Err("bug reporting is disabled".into());
    }
    let target = if target.is_empty() || target == "public" {
        PUBLIC_TARGET
    } else {
        target
    };
    if !target.starts_with("https://")
        || target.chars().any(char::is_control)
        || target
            .split('/')
            .nth(2)
            .is_none_or(|authority| authority.is_empty() || authority.contains('@'))
        || target.contains('#')
        || target.len() > 1_000
    {
        return Err("bug_report_target must be public, off, or an HTTPS URL template".into());
    }
    let mut budget = 1_000;
    loop {
        let title = encode(&shorten(title, budget.min(120)));
        let body = encode(&shorten(body, budget));
        let url = if target.contains("{title}") || target.contains("{body}") {
            target.replace("{title}", &title).replace("{body}", &body)
        } else {
            let separator = if target.contains('?') { '&' } else { '?' };
            format!("{target}{separator}template=bug_report.yml&title={title}&body={body}")
        };
        let url = format!("{url}{extra}");
        if url.len() <= URL_LIMIT {
            return Ok(url);
        }
        if budget == 0 {
            return Err("bug-report destination exceeds URL limit".into());
        }
        budget = budget.saturating_sub(50);
    }
}

pub fn crash_signature(save: &Path) -> Option<String> {
    let dir = save.parent().unwrap_or(Path::new("."));
    let mut crashes: Vec<_> = std::fs::read_dir(dir.join("crashes"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("crash-"))
        .collect();
    crashes.sort_by_key(|entry| entry.file_name());
    let path = crashes.last()?.path();
    let file = std::fs::File::open(path).ok()?;
    use std::io::Read;
    let mut text = String::new();
    file.take(64 * 1024).read_to_string(&mut text).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("signature "))
        .filter(|signature| {
            signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(str::to_owned)
}

pub fn prepare(
    save: &Path,
    config_dir: &Path,
    fields: &Fields,
    target: &str,
) -> Result<Prepared, String> {
    if target == "off" {
        return Err("bug reporting is disabled".into());
    }
    let redactor = Redactor::for_dir(config_dir);
    let clean = |text: &str| redactor.redact(text).text;
    let title = clean(&fields.title);
    let happened = clean(&fields.happened);
    let expected = clean(&fields.expected);
    let steps = clean(&fields.steps);
    if title.trim().is_empty() || happened.trim().is_empty() || expected.trim().is_empty() {
        return Err("title, what happened and expected behavior are required".into());
    }
    let signature = crash_signature(save);
    let summary = format!(
        "# {title}\n\n## What happened\n{happened}\n\n## Expected\n{expected}\n\n## Steps\n{steps}\n\nVersion: {}\nChannel: {}\nOS / arch: {} / {}\ncrash-signature: {}\n\nAttach the local report after reviewing it.\n",
        crate::update::running_version(),
        crate::update::channel(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        signature.as_deref().unwrap_or("none")
    );
    let url_summary = format!(
        "Version: {}\nChannel: {}\nOS / arch: {} / {}\ncrash-signature: {}\n\nTitle: {}\nHappened: {}\nExpected: {}\nSteps: {}\nAttach the local report after reviewing it.",
        crate::update::running_version(),
        crate::update::channel(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        signature.as_deref().unwrap_or("none"),
        shorten(&title, 120),
        shorten(&happened, 250),
        shorten(&expected, 250),
        shorten(&steps, 150)
    );
    let url = if target.is_empty() || target == "public" {
        let extra = format!(
            "&happened={}&expected={}&steps={}&version={}&platform={}&signature={}",
            encode(&shorten(&happened, 100)),
            encode(&shorten(&expected, 100)),
            encode(&shorten(&steps, 80)),
            encode(&format!(
                "{} {}",
                crate::update::running_version(),
                crate::update::channel()
            )),
            encode(&format!(
                "{} / {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )),
            signature.as_deref().unwrap_or("none")
        );
        build_url(target, &title, &url_summary, &extra)?
    } else {
        issue_url(target, &title, &url_summary)?
    };
    let search_url = signature.map(|signature| {
        format!(
            "https://github.com/Sannrox/hoshi/issues?q={}",
            encode(&format!("crash-signature: {signature}"))
        )
    });
    let mut report = support::build(save, false, 100, None, false, Some(config_dir))?;
    report.markdown = format!(
        "{summary}\n## Issue page\n{url}\n\n{}\n{}",
        search_url
            .as_ref()
            .map(|url| format!("Search existing issues: {url}\n"))
            .unwrap_or_default(),
        report.markdown
    );
    Ok(Prepared {
        report,
        url,
        search_url,
    })
}

pub fn save(prepared: &Prepared, save: &Path) -> Result<PathBuf, String> {
    let out = support::default_out(save, false);
    let file = out
        .file_name()
        .ok_or("invalid report filename")?
        .to_string_lossy();
    let out = out.with_file_name(file.replacen("hoshi-report-", "bug-", 1));
    support::write(&prepared.report, &out, false)?;
    Ok(out)
}

pub fn offline() -> bool {
    crate::update::offline(&std::env::args().collect::<Vec<_>>())
}

pub fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    let status = command
        .arg(url)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("browser could not open; use the saved URL".into())
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    use std::io::{IsTerminal, Write};
    let mut fields = Fields::default();
    let mut save_path = None;
    let mut yes = false;
    let mut local_only = offline();
    let mut print_url = false;
    let mut words = args.iter().skip(1);
    let mut command_seen = false;
    while let Some(word) = words.next() {
        match word.as_str() {
            "bug" if !command_seen => command_seen = true,
            "--save" => save_path = Some(PathBuf::from(words.next().ok_or("--save needs a path")?)),
            "--title" => fields.title = words.next().ok_or("--title needs text")?.clone(),
            "--happened" => fields.happened = words.next().ok_or("--happened needs text")?.clone(),
            "--expected" => fields.expected = words.next().ok_or("--expected needs text")?.clone(),
            "--steps" => fields.steps = words.next().ok_or("--steps needs text")?.clone(),
            "--yes" => yes = true,
            "--offline" => local_only = true,
            "--print-url" => print_url = true,
            _ if word.starts_with("--save=") => save_path = Some(PathBuf::from(&word[7..])),
            _ => return Err(format!("unexpected bug argument {word}")),
        }
    }
    for (label, value) in [
        ("Title", &mut fields.title),
        ("What happened", &mut fields.happened),
        ("Expected", &mut fields.expected),
    ] {
        if value.trim().is_empty() {
            if !std::io::stdin().is_terminal() {
                return Err(format!("{label} is required"));
            }
            print!("{label}: ");
            std::io::stdout()
                .flush()
                .map_err(|error| error.to_string())?;
            std::io::stdin()
                .read_line(value)
                .map_err(|error| error.to_string())?;
            *value = value.trim().into();
        }
    }
    let path = hoshi_storage::resolve_save_path(
        save_path.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let dir = path.parent().unwrap_or(Path::new("."));
    let target = crate::config::UserConfig::load(dir).bug_report_target;
    let prepared = prepare(&path, dir, &fields, &target)?;
    print!("{}", prepared.report.markdown);
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err("confirmation required; use --yes".into());
        }
        print!(
            "Save this report{}? [y/N] ",
            if local_only || print_url {
                ""
            } else {
                " and open the issue page"
            }
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err("bug report cancelled".into());
        }
    }
    let out = save(&prepared, &path)?;
    println!("Report and URL saved: {}", out.display());
    if !local_only
        && !print_url
        && let Err(error) = open_browser(&prepared.url)
    {
        eprintln!("{error}");
    }
    if !local_only {
        println!("{}", prepared.url);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hoshi-bug-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn issue_form_fields_match_the_bug_flow() {
        let form: serde_yaml::Value = serde_yaml::from_str(include_str!(
            "../../../.github/ISSUE_TEMPLATE/bug_report.yml"
        ))
        .unwrap();
        let ids: Vec<_> = form["body"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|field| field["id"].as_str())
            .collect();
        for field in [
            "title",
            "happened",
            "expected",
            "steps",
            "version",
            "platform",
            "signature",
            "report",
        ] {
            assert!(ids.contains(&field), "missing form field {field}");
        }
    }

    #[test]
    fn issue_url_bounds_unicode_and_supports_custom_target() {
        let url = issue_url("public", &"星".repeat(20_000), &"界".repeat(20_000)).unwrap();
        assert!(url.len() <= URL_LIMIT);
        assert!(url.starts_with(PUBLIC_TARGET));
        assert!(url.contains(&encode(TRUNCATED)));
        let custom = issue_url(
            "https://tracker.invalid/new?t={title}&b={body}",
            "a&b",
            "c d",
        )
        .unwrap();
        assert_eq!(custom, "https://tracker.invalid/new?t=a%26b&b=c%20d");
        assert!(issue_url("off", "a", "b").is_err());
    }

    #[test]
    fn multiline_config_credentials_are_redacted_from_user_fields() {
        let dir = dir();
        std::fs::write(
            dir.join("config.toml"),
            "future_secret = \"\"\"\nmultiline-test-credential\n\"\"\"\n",
        )
        .unwrap();
        let prepared = prepare(
            &dir.join("save.sqlite"),
            &dir,
            &Fields {
                title: "multiline-test-credential".into(),
                happened: "Stopped".into(),
                expected: "Continue".into(),
                steps: String::new(),
            },
            "public",
        )
        .unwrap();
        assert!(!prepared.url.contains("multiline-test-credential"));
        assert!(
            !prepared
                .report
                .markdown
                .contains("multiline-test-credential")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preparation_redacts_fields_and_preserves_crash_signature_without_writing() {
        let dir = dir();
        std::fs::write(
            dir.join("config.toml"),
            "serve_token = \"private-example-token\"\n",
        )
        .unwrap();
        let context = crate::diagnostics::Context {
            dir: dir.clone(),
            redactor: Redactor::for_dir(&dir),
            server: false,
            level: "info".into(),
        };
        crate::diagnostics::capture_crash(&context, "test panic", "game.rs:12", "function", 100)
            .unwrap();
        let signature = crash_signature(&dir.join("save.sqlite")).unwrap();
        let prepared = prepare(
            &dir.join("save.sqlite"),
            &dir,
            &Fields {
                title: "private-example-token failure".into(),
                happened: "星".repeat(20_000),
                expected: "Continue".into(),
                steps: String::new(),
            },
            "public",
        )
        .unwrap();
        assert!(!prepared.url.contains("private-example-token"));
        assert!(!prepared.report.markdown.contains("private-example-token"));
        assert!(prepared.url.contains(&signature));
        assert!(prepared.report.markdown.contains(&signature));
        assert!(prepared.search_url.unwrap().contains(&signature));
        assert!(!dir.join("reports").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
