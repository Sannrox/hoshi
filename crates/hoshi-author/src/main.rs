//! `hoshi-author events draft` — writers only; the game never runs this.

use hoshi_author::{AuthorConfig, DraftError, DraftRequest, write_draft};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("hoshi-author: {err}");
            ExitCode::from(exit_code(&err))
        }
    }
}

fn run(args: &[String]) -> Result<PathBuf, DraftError> {
    if args.first().map(String::as_str) == Some("--help") || args.is_empty() {
        return Err(DraftError::Usage(usage().into()));
    }
    if args.first().map(String::as_str) != Some("events")
        || args.get(1).map(String::as_str) != Some("draft")
    {
        return Err(DraftError::Usage(usage().into()));
    }
    let mut theme = None;
    let mut count = None;
    let mut out_dir = PathBuf::from("content/drafts");
    let mut config_path = PathBuf::from("author.toml");
    let mut endpoint_flag: Option<String> = None;
    let mut model_flag: Option<String> = None;
    let mut allow_lan_flag: Option<bool> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--theme" => {
                theme = Some(need_value(args, &mut i, "--theme")?);
            }
            "--count" => {
                let raw = need_value(args, &mut i, "--count")?;
                count =
                    Some(raw.parse::<u32>().map_err(|_| {
                        DraftError::Usage("count must be a positive integer".into())
                    })?);
            }
            "--out" => {
                out_dir = PathBuf::from(need_value(args, &mut i, "--out")?);
            }
            "--endpoint" => {
                endpoint_flag = Some(need_value(args, &mut i, "--endpoint")?);
            }
            "--model" => {
                model_flag = Some(need_value(args, &mut i, "--model")?);
            }
            "--config" => {
                config_path = PathBuf::from(need_value(args, &mut i, "--config")?);
            }
            "--allow-lan" => {
                allow_lan_flag = Some(true);
                i += 1;
            }
            "--help" => return Err(DraftError::Usage(usage().into())),
            other => {
                return Err(DraftError::Usage(format!("unknown flag {other}")));
            }
        }
    }
    let theme = theme.ok_or_else(|| DraftError::Usage("missing --theme".into()))?;
    let count = count.ok_or_else(|| DraftError::Usage("missing --count".into()))?;
    let cfg = AuthorConfig::load(&config_path);
    write_draft(&DraftRequest {
        theme,
        count,
        out_dir,
        endpoint: endpoint_flag.unwrap_or(cfg.endpoint),
        model: model_flag.unwrap_or(cfg.model),
        allow_lan: allow_lan_flag.unwrap_or(cfg.allow_lan),
    })
}

fn need_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, DraftError> {
    let value = args
        .get(*i + 1)
        .cloned()
        .ok_or_else(|| DraftError::Usage(format!("{flag} needs a value")))?;
    *i += 2;
    Ok(value)
}

fn usage() -> &'static str {
    "usage: hoshi-author events draft --theme <theme> --count <n> [--out DIR] [--endpoint URL] [--model NAME] [--allow-lan] [--config PATH]"
}

fn exit_code(err: &DraftError) -> u8 {
    match err {
        DraftError::Usage(_) | DraftError::Remote(_) | DraftError::Validate(_) => 2,
        DraftError::Io(_) => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_theme_is_usage() {
        let err = run(&[
            "events".into(),
            "draft".into(),
            "--count".into(),
            "1".into(),
        ])
        .expect_err("usage");
        assert!(matches!(err, DraftError::Usage(_)));
    }

    #[test]
    fn cli_writes_a_validated_draft() {
        let dir = std::env::temp_dir().join(format!(
            "hoshi-author-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("dir");
        let path = run(&[
            "events".into(),
            "draft".into(),
            "--theme".into(),
            "comet ice".into(),
            "--count".into(),
            "2".into(),
            "--out".into(),
            dir.to_string_lossy().into_owned(),
        ])
        .expect("draft");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("comet_ice.toml")
        );
        hoshi_author::validate_draft(&fs::read_to_string(&path).expect("read")).expect("loader");
        fs::remove_dir_all(dir).ok();
    }
}
