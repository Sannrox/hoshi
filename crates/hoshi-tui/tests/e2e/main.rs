//! Data-driven harness CLI tests. Scenario directories live beside this file.

use hoshi_core::{SCHEMA_VERSION, lock_save};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn e2e_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/e2e")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema_path() -> PathBuf {
    workspace_root().join("docs/schema/cli-v1.json")
}

fn update_golden() -> bool {
    matches!(
        std::env::var("HOSHI_UPDATE_GOLDEN").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn unique_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "hoshi-e2e-{name}-{}-{nanos}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[derive(Debug, Clone)]
enum Step {
    Comment,
    Expect(i32),
    Restart,
    Lock,
    Unlock,
    Fixture(String),
    NoSeed,
    Seed(u64),
    Reset,
    Snapshot(String),
    Command(Vec<String>),
}

fn parse_script(text: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim();
            if rest.is_empty() {
                steps.push(Step::Comment);
                continue;
            }
            let mut parts = rest.split_whitespace();
            let Some(head) = parts.next() else {
                steps.push(Step::Comment);
                continue;
            };
            match head {
                "expect" => {
                    let code = parts
                        .next()
                        .expect("# expect needs a code")
                        .parse()
                        .expect("expect code");
                    steps.push(Step::Expect(code));
                }
                "restart" => steps.push(Step::Restart),
                "lock" => steps.push(Step::Lock),
                "unlock" => steps.push(Step::Unlock),
                "fixture" => {
                    let name = parts.next().expect("# fixture needs a path").to_string();
                    steps.push(Step::Fixture(name));
                }
                "no-seed" => steps.push(Step::NoSeed),
                "seed" => {
                    let seed = parts
                        .next()
                        .expect("# seed needs a value")
                        .parse()
                        .expect("seed");
                    steps.push(Step::Seed(seed));
                }
                "reset" => steps.push(Step::Reset),
                "snapshot" => {
                    let name = parts.next().expect("# snapshot needs a name").to_string();
                    steps.push(Step::Snapshot(name));
                }
                _ => steps.push(Step::Comment),
            }
            continue;
        }
        steps.push(Step::Command(
            line.split_whitespace().map(str::to_string).collect(),
        ));
    }
    steps
}

fn parse_advance(text: &str) -> u64 {
    if let Some(n) = text.strip_suffix('s') {
        return n.parse().expect("advance seconds");
    }
    if let Some(n) = text.strip_suffix('m') {
        return n.parse::<u64>().expect("advance minutes") * 60;
    }
    if let Some(n) = text.strip_suffix('h') {
        return n.parse::<u64>().expect("advance hours") * 3600;
    }
    if let Some(n) = text.strip_suffix('d') {
        return n.parse::<u64>().expect("advance days") * 86_400;
    }
    text.parse().expect("advance seconds")
}

fn tick_secs(args: &[String]) -> Option<u64> {
    if args.first().map(String::as_str) != Some("tick") {
        return None;
    }
    let mut i = 1;
    while i < args.len() {
        if let Some(v) = args[i].strip_prefix("--advance=") {
            return Some(parse_advance(v));
        }
        if args[i] == "--advance" {
            return Some(parse_advance(&args[i + 1]));
        }
        i += 1;
    }
    None
}

fn collapsed_tick_script(steps: &[Step]) -> Option<Vec<Step>> {
    let mut total = 0u64;
    let mut saw_tick = false;
    let mut saw_restart = false;
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut passed_tick = false;
    for step in steps {
        match step {
            Step::Restart => {
                saw_restart = true;
            }
            Step::Command(args) => {
                if let Some(secs) = tick_secs(args) {
                    saw_tick = true;
                    passed_tick = true;
                    total = total.saturating_add(secs);
                    continue;
                }
                if passed_tick {
                    suffix.push(step.clone());
                } else {
                    prefix.push(step.clone());
                }
            }
            other => {
                if passed_tick {
                    suffix.push(other.clone());
                } else {
                    prefix.push(other.clone());
                }
            }
        }
    }
    if !saw_tick || !saw_restart {
        return None;
    }
    let mut out = prefix;
    out.push(Step::Command(vec![
        "tick".into(),
        "--advance".into(),
        total.to_string(),
    ]));
    out.extend(suffix);
    Some(out)
}

fn resolve_fixture(scenario: &Path, name: &str) -> PathBuf {
    let in_scenario = scenario.join(name);
    if in_scenario.exists() {
        return in_scenario;
    }
    e2e_root().join("fixtures").join(name)
}

fn copy_save(from: &Path, to: &Path) {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).expect("save parent");
    }
    fs::copy(from, to)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
}

struct Spawned {
    value: Value,
    raw: String,
    exit: i32,
}

fn spawn_cli(save: &Path, seed: Option<u64>, balance: Option<&Path>, args: &[String]) -> Spawned {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hoshi"));
    cmd.arg("--save").arg(save);
    if let Some(seed) = seed {
        cmd.arg("--seed").arg(seed.to_string());
    }
    if let Some(balance) = balance {
        cmd.arg("--balance").arg(balance);
    }
    cmd.args(args);
    cmd.env_remove("HOSHI_SAVE");
    cmd.env_remove("HOSHI_BALANCE");
    let output = cmd.output().expect("spawn hoshi");
    let raw = String::from_utf8(output.stdout).expect("cli stdout utf-8");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value: Value = serde_json::from_str(&raw).unwrap_or_else(|e| {
        panic!("cli json ({e}) stdout={raw:?} stderr={stderr}");
    });
    Spawned {
        value,
        raw,
        exit: output.status.code().unwrap_or(1),
    }
}

fn load_schema() -> Value {
    let text = fs::read_to_string(schema_path()).expect("cli schema");
    serde_json::from_str(&text).expect("cli schema json")
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_type(expected: &str, value: &Value) -> bool {
    match expected {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.as_f64().is_some(),
        other => panic!("unsupported schema type {other}"),
    }
}

fn validate_schema(schema: &Value, instance: &Value, path: &str) -> Result<(), String> {
    if let Some(constant) = schema.get("const")
        && instance != constant
    {
        return Err(format!("{path}: expected const {constant}, got {instance}"));
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(instance)
    {
        return Err(format!("{path}: {instance} is not in {options:?}"));
    }
    if let Some(types) = schema.get("type") {
        let ok = match types {
            Value::String(t) => matches_type(t, instance),
            Value::Array(list) => list
                .iter()
                .any(|t| t.as_str().is_some_and(|name| matches_type(name, instance))),
            other => panic!("schema type must be string or array, got {other}"),
        };
        if !ok {
            return Err(format!(
                "{path}: expected type {types}, got {}",
                type_name(instance)
            ));
        }
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && let Some(n) = instance.as_f64()
        && n < min
    {
        return Err(format!("{path}: {n} is below minimum {min}"));
    }
    if let Some(obj) = instance.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required {
                let key = key.as_str().expect("required key");
                if !obj.contains_key(key) {
                    return Err(format!("{path}: missing required {key}"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in props {
                if let Some(value) = obj.get(key) {
                    let child_path = format!("{path}.{key}");
                    validate_schema(child, value, &child_path)?;
                }
            }
        }
    }
    if let Some(items) = schema.get("items")
        && let Some(list) = instance.as_array()
    {
        for (i, value) in list.iter().enumerate() {
            validate_schema(items, value, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn numbers_eq(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return x == y;
    }
    a.as_f64() == b.as_f64()
}

fn json_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => numbers_eq(a, b),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|w| json_eq(v, w)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_eq(x, y))
        }
        _ => left == right,
    }
}

fn json_diff(left: &Value, right: &Value, path: &str, out: &mut Vec<String>) {
    if json_eq(left, right) {
        return;
    }
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                match (a.get(key), b.get(key)) {
                    (Some(l), Some(r)) => json_diff(l, r, &format!("{path}.{key}"), out),
                    (Some(l), None) => {
                        out.push(format!("{path}.{key}: golden {l} != actual <missing>"))
                    }
                    (None, Some(r)) => {
                        out.push(format!("{path}.{key}: golden <missing> != actual {r}"))
                    }
                    (None, None) => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!(
                    "{path}: array length golden {} != actual {}",
                    a.len(),
                    b.len()
                ));
            }
            for (i, (l, r)) in a.iter().zip(b.iter()).enumerate() {
                json_diff(l, r, &format!("{path}[{i}]"), out);
            }
        }
        _ => out.push(format!("{path}: golden {left} != actual {right}")),
    }
}

fn write_pretty(path: &Path, raw: &str) {
    fs::write(path, raw).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn compare_golden(path: &Path, raw: &str, actual: &Value, label: &str) {
    if update_golden() {
        write_pretty(path, raw);
        return;
    }
    if !path.exists() {
        panic!(
            "{label}: missing golden {} (set HOSHI_UPDATE_GOLDEN=1)",
            path.display()
        );
    }
    let expected_text = fs::read_to_string(path).expect("golden");
    let expected: Value = serde_json::from_str(&expected_text)
        .unwrap_or_else(|e| panic!("{}: golden json ({e})", path.display()));
    if json_eq(&expected, actual) {
        return;
    }
    let mut diff = Vec::new();
    json_diff(&expected, actual, "$", &mut diff);
    panic!(
        "{label}: golden mismatch {}\n{}",
        path.display(),
        diff.join("\n")
    );
}

struct RunResult {
    last: Spawned,
}

fn run_steps(scenario: &Path, save: &Path, steps: &[Step], schema: &Value) -> RunResult {
    let balance = {
        let path = scenario.join("balance.toml");
        path.exists().then_some(path)
    };
    let mut seed = Some(1u64);
    let mut expect = 0i32;
    let mut lock = None;
    let mut last: Option<Spawned> = None;
    for step in steps {
        match step {
            Step::Comment | Step::Restart => {}
            Step::Expect(code) => expect = *code,
            Step::Lock => {
                lock = Some(lock_save(save).expect("take save lock"));
            }
            Step::Unlock => {
                lock = None;
            }
            Step::Fixture(name) => {
                let src = resolve_fixture(scenario, name);
                assert!(src.exists(), "fixture missing: {}", src.display());
                copy_save(&src, save);
            }
            Step::NoSeed => seed = None,
            Step::Seed(value) => seed = Some(*value),
            Step::Reset => {
                let _ = fs::remove_file(save);
                let _ = fs::remove_file(format!("{}.lock", save.display()));
            }
            Step::Snapshot(name) => {
                let Some(spawned) = last.as_ref() else {
                    panic!("# snapshot {name} with no command");
                };
                compare_golden(
                    &scenario.join(format!("expected-{name}.json")),
                    &spawned.raw,
                    &spawned.value,
                    &format!("{} snapshot {name}", scenario.display()),
                );
            }
            Step::Command(args) => {
                let spawned = spawn_cli(save, seed, balance.as_deref(), args);
                validate_schema(schema, &spawned.value, "$").unwrap_or_else(|e| {
                    panic!("{} command {:?} schema: {e}", scenario.display(), args);
                });
                assert_eq!(
                    spawned.exit,
                    expect,
                    "{} command {:?} exit {} != {expect}: {}",
                    scenario.display(),
                    args,
                    spawned.exit,
                    spawned.raw
                );
                last = Some(spawned);
                expect = 0;
            }
        }
    }
    drop(lock);
    RunResult {
        last: last.expect("scenario produced no JSON"),
    }
}

fn scenario_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(e2e_root()).expect("e2e dir") {
        let entry = entry.expect("e2e entry");
        let path = entry.path();
        if path.is_dir() && path.join("script.txt").exists() {
            dirs.push(path);
        }
    }
    dirs.sort();
    dirs
}

fn run_scenario(dir: &Path, schema: &Value) {
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("scenario");
    let tmp = unique_dir(name);
    let save = tmp.join("save.sqlite");
    if dir.join("save.sqlite").exists() {
        copy_save(&dir.join("save.sqlite"), &save);
    }
    let script = fs::read_to_string(dir.join("script.txt")).expect("script.txt");
    let steps = parse_script(&script);
    let result = run_steps(dir, &save, &steps, schema);
    compare_golden(
        &dir.join("expected.json"),
        &result.last.raw,
        &result.last.value,
        name,
    );
    if let Some(combined) = collapsed_tick_script(&steps) {
        let combined_dir = unique_dir(&format!("{name}-combined"));
        let combined_save = combined_dir.join("save.sqlite");
        if dir.join("save.sqlite").exists() {
            copy_save(&dir.join("save.sqlite"), &combined_save);
        }
        let combined_result = run_steps(dir, &combined_save, &combined, schema);
        if !json_eq(&result.last.value, &combined_result.last.value) {
            let mut diff = Vec::new();
            json_diff(
                &result.last.value,
                &combined_result.last.value,
                "$",
                &mut diff,
            );
            panic!(
                "{name}: restarted catch-up != single tick of the same advance\n{}",
                diff.join("\n")
            );
        }
        let _ = fs::remove_dir_all(&combined_dir);
    }
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn harness_scenarios() {
    let schema = load_schema();
    let dirs = scenario_dirs();
    assert!(
        !dirs.is_empty(),
        "no scenario directories under {}",
        e2e_root().display()
    );
    let mut names: Vec<_> = dirs
        .iter()
        .filter_map(|d| d.file_name().and_then(|s| s.to_str()).map(str::to_string))
        .collect();
    names.sort();
    for required in [
        "build_research",
        "catch_up_restart",
        "expedition_raid",
        "save_locked",
        "old_save_migrate",
    ] {
        assert!(
            names.iter().any(|n| n == required),
            "missing required scenario {required}"
        );
    }
    for dir in dirs {
        run_scenario(&dir, &schema);
    }
}

#[test]
fn every_legacy_fixture_migrates() {
    let schema = load_schema();
    let fixtures = e2e_root().join("fixtures");
    let mut found = Vec::new();
    for entry in fs::read_dir(&fixtures).expect("fixtures dir") {
        let path = entry.expect("fixture").path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(rest) = name.strip_prefix('v')
            && let Some(num) = rest.strip_suffix(".sqlite")
            && num.parse::<u32>().is_ok()
        {
            found.push(path);
        }
    }
    found.sort();
    assert_eq!(
        found.len() as u32,
        SCHEMA_VERSION.saturating_sub(1),
        "expected fixtures for every schema before {SCHEMA_VERSION}"
    );
    for path in found {
        let tmp = unique_dir("legacy");
        let save = tmp.join("save.sqlite");
        copy_save(&path, &save);
        let spawned = spawn_cli(&save, None, None, &["state".into()]);
        validate_schema(&schema, &spawned.value, "$").unwrap_or_else(|e| {
            panic!("{} schema: {e}", path.display());
        });
        assert_eq!(
            spawned.exit,
            0,
            "{} exit {}: {}",
            path.display(),
            spawned.exit,
            spawned.raw
        );
        assert_eq!(spawned.value["ok"], Value::Bool(true), "{}", path.display());
        assert_eq!(
            spawned.value["save_schema"].as_u64(),
            Some(u64::from(SCHEMA_VERSION)),
            "{}",
            path.display()
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}

#[test]
fn schema_rejects_a_missing_required_field() {
    let schema = load_schema();
    let err = validate_schema(&schema, &serde_json::json!({"ok": true}), "$").unwrap_err();
    assert!(err.contains("schema_version"), "{err}");
}
