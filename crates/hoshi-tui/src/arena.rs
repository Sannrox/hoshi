//! Opt-in local bot arena. Bots speak the JSON CLI; the game never calls a model.

use crate::cli::{self, EXIT_OK};
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static RUN_LOCK: Mutex<()> = Mutex::new(());
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

pub const SCORING_VERSION: u32 = 1;
pub const BOARD_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct ArenaSpec {
    pub seed: u64,
    pub days: u32,
    pub actions: u32,
    pub timeout: Duration,
    pub bots: Vec<PathBuf>,
    pub board: PathBuf,
    pub chisei: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BotResult {
    pub name: String,
    pub status: String,
    pub error: Option<String>,
    pub score: f64,
    pub efficiency: f64,
    pub victory: f64,
    pub prestige: f64,
    pub metal_mine: u32,
    pub actions: u32,
}

pub fn run(spec: &ArenaSpec) -> Result<Value, String> {
    let _guard = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if spec.bots.is_empty() {
        return Err("arena run needs at least one bot".into());
    }
    if spec.days == 0 {
        return Err("arena --days must be at least 1".into());
    }
    let root = dir_or_dot(&spec.board);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let work = root.join(format!(
        "arena-{}-{}-{}",
        spec.seed,
        std::process::id(),
        RUN_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    if spec.chisei && !cfg!(feature = "chisei") {
        return Err("rebuild with --features chisei to run the arena on a .chisei save".into());
    }
    let mut results = Vec::new();
    for (i, bot) in spec.bots.iter().enumerate() {
        let save = if spec.chisei {
            work.join(format!("bot-{i}.chisei"))
        } else {
            work.join(format!("bot-{i}.sqlite"))
        };
        let name = bot
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("bot")
            .to_string();
        results.push(run_one(spec, bot, &name, &save));
    }
    let _ = fs::remove_dir_all(&work);
    let payload = json!({
        "schema_version": cli::SCHEMA_VERSION,
        "ok": true,
        "message": "arena",
        "scoring_version": SCORING_VERSION,
        "seed": spec.seed,
        "days": spec.days,
        "bots": results.iter().map(bot_json).collect::<Vec<_>>(),
        "markdown": markdown(&results, spec.seed, spec.days),
    });
    append_board(&spec.board, spec, &results)?;
    Ok(payload)
}

pub fn board_markdown(path: &Path) -> Result<Value, String> {
    let board = load_board(path)?;
    let md = board_to_markdown(&board);
    Ok(json!({
        "schema_version": cli::SCHEMA_VERSION,
        "ok": true,
        "message": "arena board",
        "scoring_version": SCORING_VERSION,
        "board": board,
        "markdown": md,
    }))
}

fn run_one(spec: &ArenaSpec, bot: &Path, name: &str, save: &Path) -> BotResult {
    let create = cli::run_cli(&args(&[
        "--save",
        save.to_str().unwrap_or(""),
        "--seed",
        &spec.seed.to_string(),
        "state",
    ]));
    if create.exit != EXIT_OK {
        return failed(name, "invalid", extract_err(&create.json));
    }
    let mut used = 0u32;
    for day in 1..=spec.days {
        match collect_cmds(bot, spec.timeout, day, spec) {
            Ok(cmds) => {
                for cmd in cmds {
                    if used >= spec.actions {
                        break;
                    }
                    used += 1;
                    let Some(cmd) = sanitize_cmd(cmd) else {
                        continue;
                    };
                    let mut argv = vec!["hoshi".into(), format!("--save={}", save.display())];
                    argv.extend(cmd);
                    let out = cli::run_cli(&argv);
                    if out.exit != EXIT_OK && out.exit != cli::EXIT_REFUSED {
                        return finish(
                            name,
                            save,
                            spec.days,
                            used,
                            "invalid",
                            Some(extract_err(&out.json)),
                        );
                    }
                }
            }
            Err(kind) => {
                tick_day(save);
                return finish(name, save, spec.days, used, &kind.0, Some(kind.1));
            }
        }
        tick_day(save);
    }
    finish(name, save, spec.days, used, "ok", None)
}

fn tick_day(save: &Path) {
    let _ = cli::run_cli(&args(&[
        "--save",
        save.to_str().unwrap_or(""),
        "tick",
        "--advance",
        "1d",
    ]));
}

fn collect_cmds(
    bot: &Path,
    timeout: Duration,
    day: u32,
    spec: &ArenaSpec,
) -> Result<Vec<Vec<String>>, (String, String)> {
    let mut child = spawn_bot(bot)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ("crash".into(), e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let hint = json!({
            "day": day,
            "days": spec.days,
            "actions": spec.actions,
            "seed": spec.seed,
        });
        let _ = writeln!(stdin, "{hint}");
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if !status.success() {
                    return Err(("crash".into(), format!("exit {status}")));
                }
                return parse_cmds(&stdout);
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(("timeout".into(), "bot exceeded the step timeout".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(("crash".into(), e.to_string())),
        }
    }
}

fn sanitize_cmd(cmd: Vec<String>) -> Option<Vec<String>> {
    let name = cmd.first()?.as_str();
    if matches!(name, "tick" | "arena" | "cheat") {
        return None;
    }
    if cmd.iter().any(|a| {
        a == "--save"
            || a.starts_with("--save=")
            || a == "--seed"
            || a.starts_with("--seed=")
            || a == "--balance"
            || a.starts_with("--balance=")
            || a == "--arena"
            || a == "--cheats"
    }) {
        return None;
    }
    Some(cmd)
}

fn parse_cmds(stdout: &str) -> Result<Vec<Vec<String>>, (String, String)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|_| ("invalid".into(), format!("malformed JSON: {line}")))?;
        let cmd = value
            .get("cmd")
            .and_then(Value::as_array)
            .ok_or_else(|| ("invalid".into(), "missing cmd array".into()))?;
        let mut parts = Vec::new();
        for item in cmd {
            let part = item
                .as_str()
                .ok_or_else(|| ("invalid".into(), "cmd elements must be strings".into()))?;
            parts.push(part.to_string());
        }
        if parts.is_empty() {
            return Err(("invalid".into(), "empty cmd".into()));
        }
        if sanitize_cmd(parts.clone()).is_none() {
            return Err(("invalid".into(), format!("forbidden command {}", parts[0])));
        }
        out.push(parts);
    }
    Ok(out)
}

fn finish(
    name: &str,
    save: &Path,
    days: u32,
    actions: u32,
    status: &str,
    error: Option<String>,
) -> BotResult {
    let state = cli::run_cli(&args(&["--save", save.to_str().unwrap_or(""), "state"]));
    let value: Value = serde_json::from_str(&state.json).unwrap_or(json!({}));
    score_state(name, &value, days, actions, status, error)
}

fn score_state(
    name: &str,
    state: &Value,
    days: u32,
    actions: u32,
    status: &str,
    error: Option<String>,
) -> BotResult {
    if state["modified"].as_bool() != Some(false) {
        return failed(
            name,
            "refused",
            "modified or unverifiable save is ineligible".into(),
        );
    }
    let prestige = state["prestige"]["score"].as_f64().unwrap_or(0.0);
    let metal_mine = state["buildings"]["metal_mine"].as_u64().unwrap_or(0) as u32;
    let crystal_mine = state["buildings"]["crystal_mine"].as_u64().unwrap_or(0) as u32;
    let solar = state["buildings"]["solar_plant"].as_u64().unwrap_or(0) as u32;
    let colonies = state["planets"]
        .as_array()
        .map(|rows| rows.len().saturating_sub(1) as f64)
        .unwrap_or(0.0);
    let metal = state["resources"]["iron"].as_f64().unwrap_or(0.0);
    let eligible = state["prestige"]["eligible"].as_bool().unwrap_or(false);
    let mut score = prestige
        + 10.0 * f64::from(metal_mine)
        + 5.0 * f64::from(crystal_mine)
        + 3.0 * f64::from(solar)
        + 50.0 * colonies
        + metal / 1000.0;
    if status != "ok" {
        score = 0.0;
    }
    let efficiency = if days == 0 {
        0.0
    } else {
        score / f64::from(days)
    };
    let victory = if eligible {
        1.0
    } else {
        (score / 10_000.0).clamp(0.0, 0.99)
    };
    BotResult {
        name: name.into(),
        status: status.into(),
        error,
        score,
        efficiency,
        victory,
        prestige,
        metal_mine,
        actions,
    }
}

fn failed(name: &str, status: &str, error: String) -> BotResult {
    BotResult {
        name: name.into(),
        status: status.into(),
        error: Some(error),
        score: 0.0,
        efficiency: 0.0,
        victory: 0.0,
        prestige: 0.0,
        metal_mine: 0,
        actions: 0,
    }
}

fn bot_json(row: &BotResult) -> Value {
    json!({
        "name": row.name,
        "status": row.status,
        "error": row.error,
        "score": row.score,
        "efficiency": row.efficiency,
        "victory": row.victory,
        "prestige": row.prestige,
        "metal_mine": row.metal_mine,
        "actions": row.actions,
    })
}

fn markdown(results: &[BotResult], seed: u64, days: u32) -> String {
    let mut lines = vec![
        format!("# Arena seed {seed} ({days}d, scoring v{SCORING_VERSION})"),
        String::new(),
        "| Bot | Status | Score | Efficiency | Victory | Mines |".into(),
        "| --- | --- | --- | --- | --- | --- |".into(),
    ];
    let mut ranked: Vec<&BotResult> = results.iter().collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for row in ranked {
        lines.push(format!(
            "| {} | {} | {:.1} | {:.2} | {:.2} | {} |",
            row.name, row.status, row.score, row.efficiency, row.victory, row.metal_mine
        ));
    }
    lines.join("\n")
}

fn append_board(path: &Path, spec: &ArenaSpec, results: &[BotResult]) -> Result<(), String> {
    let mut board = match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("arena board is not valid JSON: {e}"))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => json!({
            "schema_version": BOARD_SCHEMA,
            "scoring_version": SCORING_VERSION,
            "runs": []
        }),
        Err(err) => return Err(err.to_string()),
    };
    let runs = board
        .get_mut("runs")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "arena board is missing a runs array".to_string())?;
    runs.push(json!({
        "seed": spec.seed,
        "days": spec.days,
        "scoring_version": SCORING_VERSION,
        "bots": results.iter().map(bot_json).collect::<Vec<_>>(),
    }));
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&board).map_err(|e| e.to_string())?
        ),
    )
    .map_err(|e| e.to_string())
}

fn load_board(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn board_to_markdown(board: &Value) -> String {
    let mut lines = vec![
        format!("# Arena leaderboard (scoring v{SCORING_VERSION})"),
        String::new(),
        "| Seed | Days | Scoring | Bot | Status | Score |".into(),
        "| --- | --- | --- | --- | --- | --- |".into(),
    ];
    if let Some(runs) = board.get("runs").and_then(Value::as_array) {
        for run in runs {
            let seed = run["seed"].as_u64().unwrap_or(0);
            let days = run["days"].as_u64().unwrap_or(0);
            let scoring = run["scoring_version"]
                .as_u64()
                .unwrap_or(u64::from(SCORING_VERSION));
            if let Some(bots) = run["bots"].as_array() {
                for bot in bots {
                    lines.push(format!(
                        "| {seed} | {days} | v{scoring} | {} | {} | {:.1} |",
                        bot["name"].as_str().unwrap_or("bot"),
                        bot["status"].as_str().unwrap_or("?"),
                        bot["score"].as_f64().unwrap_or(0.0)
                    ));
                }
            }
        }
    }
    lines.join("\n")
}

fn extract_err(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| raw.chars().take(120).collect())
}

fn spawn_bot(bot: &Path) -> Command {
    let mut cmd = match bot.extension().and_then(|s| s.to_str()) {
        Some("py") => {
            let mut cmd = Command::new("python3");
            cmd.arg(bot);
            cmd
        }
        Some("sh") => {
            let mut cmd = Command::new("sh");
            cmd.arg(bot);
            cmd
        }
        _ => Command::new(bot),
    };
    drop_inherited_fds(&mut cmd);
    cmd
}

fn drop_inherited_fds(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Child must not inherit save-lock fds. On Linux flock stays held
        // until every copied descriptor is closed.
        unsafe {
            cmd.pre_exec(|| {
                close_extra_fds();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

#[cfg(unix)]
fn close_extra_fds() {
    unsafe {
        let max = getdtablesize();
        if max > 3 {
            for fd in 3..max {
                libc_close(fd);
            }
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn getdtablesize() -> i32;
    #[link_name = "close"]
    fn libc_close(fd: i32) -> i32;
}

fn dir_or_dot(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    let mut out = vec!["hoshi".into()];
    out.extend(parts.iter().map(|s| (*s).to_string()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn modified_save_never_enters_scoring() {
        let result = score_state(
            "bot",
            &serde_json::json!({"modified": true, "prestige": {"score": 999999}}),
            1,
            1,
            "ok",
            None,
        );
        assert_eq!(result.status, "refused");
        assert_eq!(result.score, 0.0);
        assert_eq!(result.victory, 0.0);
    }

    #[test]
    fn dir_or_dot_uses_cwd_when_the_board_has_no_parent() {
        assert_eq!(
            dir_or_dot(Path::new("arena-board.json")),
            PathBuf::from(".")
        );
        assert_eq!(
            dir_or_dot(Path::new("runs/board.json")),
            PathBuf::from("runs")
        );
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("hoshi-arena-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_bot(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn sample(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/bots")
            .join(name)
    }

    fn spec(dir: &Path, seed: u64, bots: Vec<PathBuf>, timeout_ms: u64) -> ArenaSpec {
        ArenaSpec {
            seed,
            days: 1,
            actions: 6,
            timeout: Duration::from_millis(timeout_ms),
            bots,
            board: dir.join("board.json"),
            chisei: false,
        }
    }

    fn scores(value: &Value) -> Vec<(String, String, i64)> {
        value["bots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap().to_string(),
                    row["status"].as_str().unwrap().to_string(),
                    (row["score"].as_f64().unwrap() * 1000.0).round() as i64,
                )
            })
            .collect()
    }

    #[test]
    fn two_sample_bots_are_reproducible_on_the_same_seed() {
        let sh = sample("greedy.sh");
        let py = sample("greedy.py");
        assert!(sh.is_file(), "{}", sh.display());
        let dir = unique_dir("repro");
        let mut bots = vec![sh.clone(), sh];
        if Command::new("python3")
            .arg("-c")
            .arg("print(1)")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            bots.push(py);
        }
        let spec = spec(&dir, 7, bots, 2_000);
        let a = run(&spec).expect("first run");
        let b = run(&spec).expect("second run");
        assert_eq!(scores(&a), scores(&b));
        assert!(a["markdown"].as_str().unwrap().contains("greedy.sh"));
        assert_eq!(a["scoring_version"], SCORING_VERSION);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_timeout_and_invalid_bots_are_isolated() {
        let dir = unique_dir("isolate");
        let greedy = sample("greedy.sh");
        let crash = write_bot(&dir, "crash.sh", "#!/bin/sh\nexit 1\n");
        let timeout = write_bot(&dir, "timeout.sh", "#!/bin/sh\nsleep 2\n");
        let invalid = write_bot(
            &dir,
            "invalid.sh",
            "#!/bin/sh\necho '{\"cmd\":[\"nope\"]}'\n",
        );
        let malformed = write_bot(&dir, "malformed.sh", "#!/bin/sh\necho not-json\n");
        let tick = write_bot(
            &dir,
            "tick.sh",
            "#!/bin/sh\necho '{\"cmd\":[\"tick\",\"--advance\",\"1d\"]}'\n",
        );
        let spec = spec(
            &dir,
            11,
            vec![greedy, crash, timeout, invalid, malformed, tick],
            // Well under the timeout bot's 2 s sleep, but long enough that a
            // busy machine starting sh does not turn a crash into a timeout.
            1_000,
        );
        let value = run(&spec).expect("arena");
        let rows = value["bots"].as_array().unwrap();
        assert_eq!(rows[0]["name"], "greedy.sh");
        assert_eq!(rows[0]["status"], "ok");
        assert!(rows[0]["score"].as_f64().unwrap() > 0.0);
        assert_eq!(rows[1]["status"], "crash");
        assert_eq!(rows[1]["score"], 0.0);
        assert_eq!(rows[2]["status"], "timeout");
        assert_eq!(rows[2]["score"], 0.0);
        assert_eq!(rows[3]["status"], "invalid");
        assert_eq!(rows[3]["score"], 0.0);
        assert_eq!(rows[4]["status"], "invalid");
        assert_eq!(rows[5]["status"], "invalid");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaderboard_appends_seeds_and_exports_markdown() {
        let dir = unique_dir("board");
        let greedy = sample("greedy.sh");
        let first = spec(&dir, 1, vec![greedy.clone()], 2_000);
        run(&first).unwrap();
        let second = spec(&dir, 2, vec![greedy], 2_000);
        run(&second).unwrap();
        let board = board_markdown(&first.board).unwrap();
        let md = board["markdown"].as_str().unwrap();
        assert!(md.contains("| 1 |"));
        assert!(md.contains("| 2 |"));
        assert!(md.contains("greedy.sh"));
        assert!(md.contains("v1"));
        assert_eq!(board["board"]["runs"].as_array().unwrap().len(), 2);
        assert_eq!(
            board["board"]["runs"][0]["scoring_version"],
            SCORING_VERSION
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_board_is_not_overwritten() {
        let dir = unique_dir("bad-board");
        let greedy = sample("greedy.sh");
        let spec = spec(&dir, 3, vec![greedy], 2_000);
        fs::write(&spec.board, "{not json").unwrap();
        let err = run(&spec).expect_err("malformed board");
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(fs::read_to_string(&spec.board).unwrap(), "{not json");
        fs::write(&spec.board, "{\"schema_version\":1}\n").unwrap();
        let err = run(&spec).expect_err("board without runs");
        assert!(err.contains("runs array"), "{err}");
        assert!(
            fs::read_to_string(&spec.board)
                .unwrap()
                .contains("schema_version")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
