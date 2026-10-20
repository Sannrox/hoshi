//! Human help and generated references. Catalog entries never execute actions.

use std::collections::{BTreeMap, BTreeSet};

macro_rules! catalog_commands {
    ($(#[help($name:literal, $summary:literal, $usage:literal, $example:literal)]
        $variant:ident $({$($fields:tt)*})?,)*) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        enum Command { $($variant $({$($fields)*})?,)* }

        pub(crate) const COMMAND_HELP: &[crate::help::Topic] = &[
            $(crate::help::Topic::cli($name, $summary, $usage, $example),)*
        ];
    };
}
pub(crate) use catalog_commands;

#[derive(Clone, Copy)]
pub(crate) struct Topic {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub example: &'static str,
    pub area: &'static str,
    pub planned: bool,
    pub command_prefix: bool,
}

impl Topic {
    pub const fn cli(
        name: &'static str,
        summary: &'static str,
        usage: &'static str,
        example: &'static str,
    ) -> Self {
        Self {
            name,
            summary,
            usage,
            example,
            area: "cli",
            planned: false,
            command_prefix: true,
        }
    }

    fn details(self) -> &'static str {
        if self.planned {
            "Planned command. This build does not execute it."
        } else {
            match self.area {
                "cli" => {
                    "Uses the selected save and the same core rules as interactive play. Results use the versioned JSON envelope. A refused action does not bypass prerequisites or fair-play restrictions."
                }
                "serve" => {
                    "Wraps one locked local save. Loopback binding is the default; authenticated remote actions use the same core rules. No automatic update request is made."
                }
                "install" => {
                    "Available in supported release builds. Installation settings do not alter game rules or save contents."
                }
                _ => {
                    "Human-readable output. Help never opens a save, takes its lock, or starts the simulation."
                }
            }
        }
    }

    fn environment(self) -> &'static str {
        match self.area {
            "serve" => {
                "HOSHI_SAVE, HOSHI_BALANCE, HOSHI_SERVE_TOKEN, HOSHI_SERVE_TOKEN_FILE, HOSHI_SERVE_PUSH, HOSHI_SERVE_PUSH_ENDPOINT, HOSHI_SERVE_VAPID_PRIVATE, HOSHI_SERVE_VAPID_SUBJECT, HOSHI_SERVE_OIDC, HOSHI_SERVE_OIDC_ISSUER, HOSHI_SERVE_OIDC_CLIENT_ID, HOSHI_SERVE_OIDC_AUTHORIZATION, HOSHI_SERVE_OIDC_TOKEN"
            }
            "install" => {
                "HOSHI_VERSION, HOSHI_CHANNEL, HOSHI_OFFLINE (planned), HOSHI_SKIP_VERSION_CHECK (planned)"
            }
            "tui" => "HOSHI_SAVE, HOSHI_BALANCE, HOSHI_PORTRAIT_PROTOCOL, NO_COLOR",
            "cli" => "HOSHI_SAVE, HOSHI_BALANCE",
            _ => "None required. See the global environment reference below.",
        }
    }
}

const EXTRA: &[Topic] = &[
    Topic {
        name: "help",
        summary: "Read command help or export a reference.",
        usage: "help [TOPIC...] | help --man | help --completions bash|zsh|fish | help --markdown",
        example: "help state",
        area: "help",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "tui",
        summary: "Open the title screen and interactive game.",
        usage: "[--continue] [--save PATH] [--balance PATH] [--no-portraits] [--commentary] [--advisor]",
        example: "--continue",
        area: "tui",
        planned: false,
        command_prefix: false,
    },
    Topic {
        name: "version",
        summary: "Print the running build version.",
        usage: "--version | -V",
        example: "--version",
        area: "help",
        planned: false,
        command_prefix: false,
    },
    Topic {
        name: "new",
        summary: "Create a game with a seed or seed code.",
        usage: "new --seed SEED [--save PATH]",
        example: "new --seed 1 --save game.sqlite",
        area: "tui",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "season",
        summary: "Inspect seasons or enter a seasonal galaxy.",
        usage: "season [start RULESET [--save PATH]]",
        example: "season",
        area: "tui",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "simulate",
        summary: "Print a deterministic pacing report.",
        usage: "simulate [--days N]",
        example: "simulate --days 3",
        area: "cli",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "post",
        summary: "Post a bridge activity event (bridge feature required).",
        usage: "post TYPE SOURCE [PAYLOAD-JSON]",
        example: "post --help",
        area: "cli",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "serve",
        summary: "Serve a versioned HTTP and WebSocket API for one save.",
        usage: "serve [--save PATH] [--bind ADDRESS:PORT] [--config DIR]",
        example: "serve --save game.sqlite",
        area: "serve",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "update",
        summary: "Update the installed game (planned).",
        usage: "update (planned)",
        example: "help update",
        area: "install",
        planned: true,
        command_prefix: true,
    },
    Topic {
        name: "changelog",
        summary: "Read bundled release notes (planned).",
        usage: "changelog (planned)",
        example: "help changelog",
        area: "install",
        planned: true,
        command_prefix: true,
    },
    Topic {
        name: "market contract list",
        summary: "List market contracts.",
        usage: "market contract [list]",
        example: "market contract list",
        area: "cli",
        planned: false,
        command_prefix: true,
    },
    Topic {
        name: "automation add transfer",
        summary: "Add a stock-triggered transfer rule.",
        usage: "automation add CONDITION PLANET RESOURCE AMOUNT transfer FROM TO RESOURCE AMOUNT",
        example: "automation add stock_above home iron 100 transfer home 1:1:2 iron 10",
        area: "cli",
        planned: false,
        command_prefix: false,
    },
];

pub(crate) struct Flag {
    pub name: &'static str,
    pub value: &'static str,
    pub summary: &'static str,
}

const FLAGS: &[Flag] = &[
    Flag {
        name: "--help",
        value: "",
        summary: "Human help (-h also works).",
    },
    Flag {
        name: "--version",
        value: "",
        summary: "Build version (-V also works).",
    },
    Flag {
        name: "--save",
        value: "PATH",
        summary: "Select a save.",
    },
    Flag {
        name: "--balance",
        value: "PATH",
        summary: "Override balance configuration.",
    },
    Flag {
        name: "--seed",
        value: "SEED",
        summary: "Initialize a deterministic harness or new-game seed.",
    },
    Flag {
        name: "--continue",
        value: "",
        summary: "Continue the current interactive slot.",
    },
    Flag {
        name: "--no-portraits",
        value: "",
        summary: "Use ASCII portraits.",
    },
    Flag {
        name: "--commentary",
        value: "",
        summary: "Enable replay commentary.",
    },
    Flag {
        name: "--advisor",
        value: "",
        summary: "Enable optional local advice.",
    },
    Flag {
        name: "--json",
        value: "",
        summary: "Explicit JSON CLI mode.",
    },
    Flag {
        name: "--arena",
        value: "",
        summary: "Opt in to the bot arena.",
    },
    Flag {
        name: "--self-play",
        value: "",
        summary: "Opt in to balance self-play.",
    },
    Flag {
        name: "--demo",
        value: "",
        summary: "Opt in to the governance demonstration.",
    },
    Flag {
        name: "--bind",
        value: "ADDRESS:PORT",
        summary: "Server listen address (serve only).",
    },
    Flag {
        name: "--config",
        value: "DIR",
        summary: "Server configuration directory (serve only).",
    },
];

const ENV: &[(&str, &str)] = &[
    ("HOSHI_SAVE", "Save path when --save is absent."),
    (
        "HOSHI_BALANCE",
        "Balance configuration path when --balance is absent.",
    ),
    ("HOSHI_BUILD", "Build-time version annotation."),
    ("HOSHI_PORTRAIT_PROTOCOL", "Portrait protocol selection."),
    ("NO_COLOR", "Disable terminal colors."),
    (
        "HOSHI_SERVE_TOKEN",
        "Runtime bearer token for the selected save.",
    ),
    (
        "HOSHI_SERVE_TOKEN_FILE",
        "Read the runtime bearer token from this file.",
    ),
    ("HOSHI_SERVE_PUSH", "Enable optional web push."),
    (
        "HOSHI_SERVE_PUSH_ENDPOINT",
        "Override the opt-in push endpoint.",
    ),
    (
        "HOSHI_SERVE_VAPID_PRIVATE",
        "Runtime push signing key; never stored in the save.",
    ),
    ("HOSHI_SERVE_VAPID_SUBJECT", "Push contact subject."),
    ("HOSHI_SERVE_OIDC", "Enable optional identity login."),
    ("HOSHI_SERVE_OIDC_ISSUER", "Identity issuer."),
    ("HOSHI_SERVE_OIDC_CLIENT_ID", "Identity client identifier."),
    (
        "HOSHI_SERVE_OIDC_AUTHORIZATION",
        "Identity authorization endpoint.",
    ),
    ("HOSHI_SERVE_OIDC_TOKEN", "Identity token endpoint."),
    ("HOSHI_VERSION", "Install script version selection."),
    ("HOSHI_CHANNEL", "Install script channel selection."),
    (
        "HOSHI_OFFLINE",
        "Planned update-check opt-out; automatic update requests are not implemented in this build.",
    ),
    (
        "HOSHI_SKIP_VERSION_CHECK",
        "Planned update-check opt-out; automatic update requests are not implemented in this build.",
    ),
];

const EXIT_CODES: &str =
    "0 success; 2 invalid input; 3 action refused; 4 save locked; 5 internal or save error.";

pub(crate) fn topics() -> Vec<Topic> {
    let mut topics: Vec<_> = crate::cli::COMMAND_HELP
        .iter()
        .chain(EXTRA)
        .copied()
        .collect();
    topics.sort_by_key(|t| t.name);
    topics
}

fn roots() -> BTreeSet<&'static str> {
    topics()
        .into_iter()
        .filter(|topic| topic.command_prefix)
        .map(|t| t.name.split_whitespace().next().unwrap_or(t.name))
        .collect()
}

fn summary() -> String {
    let mut out = String::from(
        "hoshi - terminal space strategy\n\nUsage: hoshi [TUI FLAGS]\n       hoshi [--save PATH] COMMAND [ARGS]\n       hoshi help [TOPIC...]\n\nCommands:\n",
    );
    for root in roots() {
        let all = topics();
        let topic = all
            .iter()
            .find(|t| t.name == root)
            .or_else(|| all.iter().find(|t| t.name.starts_with(&format!("{root} "))));
        if let Some(topic) = topic {
            out.push_str(&format!("  {root:<14} {}\n", topic.summary));
        }
    }
    out.push_str("\nFlags (availability depends on the command):\n");
    for flag in FLAGS {
        out.push_str(&format!(
            "  {} {}  {}\n",
            flag.name, flag.value, flag.summary
        ));
    }
    out.push_str("\nEnvironment:\n");
    for (name, description) in ENV {
        out.push_str(&format!("  {name}: {description}\n"));
    }
    out.push_str(&format!(
        "\nExit codes: {EXIT_CODES}\n\nRun hoshi help <topic> for usage and examples.\n"
    ));
    out
}

fn topic_help(name: &str) -> Result<String, String> {
    let all = topics();
    let matches: Vec<_> = all
        .iter()
        .filter(|t| t.name == name || t.name.starts_with(&format!("{name} ")))
        .collect();
    if matches.is_empty() {
        let prefix = name.chars().next();
        let suggestions: Vec<_> = roots()
            .into_iter()
            .filter(|s| s.chars().next() == prefix)
            .take(5)
            .collect();
        let suggestions = if suggestions.is_empty() {
            "state, build, fleet, serve, help".into()
        } else {
            suggestions.join(", ")
        };
        return Err(format!("Unknown help topic: {name}\nTry: {suggestions}\n"));
    }
    let mut out = String::new();
    for topic in matches {
        out.push_str(&format!("{}\n{}\n\nUsage: hoshi {}\n\n{}\n\nExample:\n  hoshi {}\n\nEnvironment: {}\nExit codes: {EXIT_CODES}\n\n", topic.name, topic.summary, topic.usage, topic.details(), topic.example, topic.environment()));
    }
    Ok(out)
}

/// Intercepts help before routing, save access, or runtime configuration.
pub(crate) fn invocation(args: &[String]) -> Option<(String, i32)> {
    let mut start = 1;
    while start < args.len() {
        let word = &args[start];
        if FLAGS.iter().any(|f| f.name == word && !f.value.is_empty()) {
            start += 2;
        } else if word.starts_with('-') {
            start += 1;
        } else {
            break;
        }
    }
    let explicit = args.get(start).is_some_and(|s| s == "help");
    if !explicit && !args.iter().skip(1).any(|s| s == "--help" || s == "-h") {
        return None;
    }
    if explicit {
        match args.get(start + 1).map(String::as_str) {
            Some("--man") if args.len() == start + 2 => return Some((man(), 0)),
            Some("--markdown") if args.len() == start + 2 => return Some((markdown(), 0)),
            Some("--completions") if args.len() == start + 3 => {
                return Some(match completions(&args[start + 2]) {
                    Ok(s) => (s, 0),
                    Err(s) => (s, 2),
                });
            }
            Some(s) if s.starts_with('-') && s != "--help" && s != "-h" => return Some((
                "Usage: hoshi help [TOPIC...] | --man | --markdown | --completions bash|zsh|fish\n"
                    .into(),
                2,
            )),
            _ => {}
        }
    }
    let mut words = Vec::new();
    let mut i = if explicit { start + 1 } else { start };
    while i < args.len() {
        let word = &args[i];
        if FLAGS.iter().any(|f| f.name == word && !f.value.is_empty()) {
            i += 2;
            continue;
        }
        if word.starts_with('-') {
            i += 1;
            continue;
        }
        words.push(word.as_str());
        i += 1;
    }
    if words.is_empty() {
        return Some((summary(), 0));
    }
    let name = words.join(" ");
    let result = if explicit {
        topic_help(&name)
    } else {
        // Positional operands are not part of a command topic.
        (1..=words.len())
            .rev()
            .find_map(|n| topic_help(&words[..n].join(" ")).ok())
            .ok_or_else(|| format!("Unknown help topic: {}\nTry: hoshi help\n", words[0]))
    };
    Some(match result {
        Ok(s) => (s, 0),
        Err(s) => (s, 2),
    })
}

pub(crate) fn markdown() -> String {
    let mut out = String::from(
        "# Command reference\n\nGenerated by `hoshi help --markdown`. Edit the command catalog, then run\n`scripts/generate-help.sh`; do not edit generated files.\n\n",
    );
    out.push_str("Bare `hoshi` opens the title screen. `hoshi --help`, `hoshi -h`, and\n`hoshi help` print human help without accessing a save. CLI commands retain\ntheir JSON output. `hoshi help TOPIC` and `hoshi COMMAND --help` show details.\n\n");
    for t in topics() {
        out.push_str(&format!("## {}\n\n{}\n\n```text\nhoshi {}\n```\n\n{}\n\nExample: `hoshi {}`\n\nEnvironment: {}\n\nArea: {}.{}\n\n", t.name, t.summary, t.usage, t.details(), t.example, t.environment(), t.area, if t.planned { " Planned." } else { "" }));
    }
    out.push_str("## Flags\n\n");
    for f in FLAGS {
        out.push_str(&format!("- `{} {}`: {}\n", f.name, f.value, f.summary));
    }
    out.push_str("\n## Environment\n\n");
    for (name, description) in ENV {
        out.push_str(&format!("- `{name}`: {description}\n"));
    }
    out.push_str(&format!("\n## Exit codes\n\n{EXIT_CODES}\n"));
    out
}

fn roff(text: &str) -> String {
    text.replace('\\', "\\e")
        .replace('-', "\\-")
        .replace('"', "\\(dq")
        .lines()
        .map(|line| {
            if line.starts_with('.') || line.starts_with('\'') {
                format!("\\&{line}")
            } else {
                line.into()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn man() -> String {
    let mut out = String::from(
        ".TH HOSHI 1\n.SH NAME\nhoshi \\- terminal space strategy\n.SH SYNOPSIS\n.B hoshi\n[flags] [command] [arguments]\n.SH DESCRIPTION\nBare hoshi opens the title screen. Help never opens a save.\n.SH COMMANDS\n",
    );
    for t in topics() {
        out.push_str(&format!(".SS {}\n{}\n.PP\n{}\n.PP\n.nf\nhoshi {}\n.fi\n.PP\nExample:\n.nf\nhoshi {}\n.fi\n.PP\nEnvironment: {}\n", roff(t.name), roff(t.summary), roff(t.details()), roff(t.usage), roff(t.example), roff(t.environment())));
    }
    out.push_str(".SH FLAGS\n");
    for f in FLAGS {
        out.push_str(&format!(
            ".TP\n.B {} {}\n{}\n",
            roff(f.name),
            roff(f.value),
            roff(f.summary)
        ));
    }
    out.push_str(".SH ENVIRONMENT\n");
    for (name, description) in ENV {
        out.push_str(&format!(".TP\n.B {name}\n{}\n", roff(description)));
    }
    out.push_str(&format!(".SH EXIT STATUS\n{}\n", roff(EXIT_CODES)));
    out
}

fn completion_tree() -> BTreeMap<String, BTreeSet<String>> {
    let mut tree = BTreeMap::<String, BTreeSet<String>>::new();
    for topic in topics() {
        for prefix in ["", "help "] {
            if prefix.is_empty() && (topic.planned || !topic.command_prefix) {
                continue;
            }
            let path = format!("{prefix}{}", topic.name);
            let mut parent = Vec::new();
            for word in path.split_whitespace() {
                tree.entry(parent.join(" "))
                    .or_default()
                    .insert(word.into());
                parent.push(word);
            }
        }
    }
    tree
}

pub(crate) fn completions(shell: &str) -> Result<String, String> {
    let flags = FLAGS.iter().map(|f| f.name).collect::<Vec<_>>().join(" ");
    let valued = FLAGS
        .iter()
        .filter(|f| !f.value.is_empty())
        .map(|f| f.name)
        .collect::<Vec<_>>()
        .join("|");
    let path_flags = FLAGS
        .iter()
        .filter(|f| ["PATH", "DIR"].contains(&f.value))
        .map(|f| f.name)
        .collect::<Vec<_>>()
        .join("|");
    let tree = completion_tree();
    let mut cases = String::new();
    for (prefix, words) in &tree {
        cases.push_str(&format!(
            "    '{prefix}') candidates='{} {flags}' ;;\n",
            words.iter().cloned().collect::<Vec<_>>().join(" ")
        ));
    }
    match shell {
        "bash" => Ok(format!(
            "# Generated by hoshi help --completions bash\n_hoshi() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\" context='' candidates='' word candidate skip=0 i previous=\"${{COMP_WORDS[COMP_CWORD-1]}}\"\n  [[ \"$previous\" == = ]] && previous=\"${{COMP_WORDS[COMP_CWORD-2]}}\"\n  case \"$previous\" in\n    {path_flags}) compopt -o filenames 2>/dev/null || :; COMPREPLY=(); while IFS= read -r candidate; do [[ -d \"$candidate\" ]] && candidate+=/; COMPREPLY+=(\"$candidate\"); done < <(compgen -f -- \"$cur\"); return ;;\n  esac\n  for ((i=1; i<COMP_CWORD; i++)); do\n    word=\"${{COMP_WORDS[i]}}\"\n    if ((skip)); then\n      if [[ \"$word\" == = ]]; then continue; fi\n      skip=0; continue\n    fi\n    case \"$word\" in\n      {valued}) skip=1 ;;\n      -*) ;;\n      *) context=\"${{context:+$context }}$word\" ;;\n    esac\n  done\n  case \"$context\" in\n{cases}    *) candidates='{flags}' ;;\n  esac\n  COMPREPLY=( $(compgen -W \"$candidates\" -- \"$cur\") )\n}}\ncomplete -o default -o bashdefault -F _hoshi hoshi\n"
        )),
        "zsh" => Ok(format!(
            "#compdef hoshi\n# Generated by hoshi help --completions zsh\n  local context='' candidates='' word skip=0 i\n  case \"${{words[CURRENT-1]}}\" in\n    {path_flags}) _files; return ;;\n  esac\n  for ((i=2; i<CURRENT; i++)); do\n    word=\"${{words[i]}}\"\n    if ((skip)); then skip=0; continue; fi\n    case \"$word\" in\n      {valued}) skip=1 ;;\n      -*) ;;\n      *) context=\"${{context:+$context }}$word\" ;;\n    esac\n  done\n  case \"$context\" in\n{cases}    *) candidates='{flags}' ;;\n  esac\n  compadd -- ${{=candidates}}\n"
        )),
        "fish" => {
            let valued_fish = FLAGS
                .iter()
                .filter(|f| !f.value.is_empty())
                .map(|f| f.name)
                .collect::<Vec<_>>()
                .join(" ");
            let mut out = format!(
                "# Generated by hoshi help --completions fish\nfunction __hoshi_context\n  set -l tokens (commandline -opc)\n  set -e tokens[1]\n  set -l context\n  set -l skip_value 0\n  for word in $tokens\n    if test $skip_value -eq 1\n      set skip_value 0\n      continue\n    end\n    switch $word\n      case {valued_fish}\n        set skip_value 1\n      case '-*'\n      case '*'\n        set -a context $word\n    end\n  end\n  set -l current (string join ' ' -- $context)\n  test \"$current\" = \"$argv[1]\"\nend\ncomplete -c hoshi -f\n"
            );
            for f in FLAGS {
                out.push_str(&format!(
                    "complete -c hoshi -l {}{} -d '{}'\n",
                    f.name.trim_start_matches('-'),
                    if f.value.is_empty() {
                        ""
                    } else if ["PATH", "DIR"].contains(&f.value) {
                        " -r -F"
                    } else {
                        " -r"
                    },
                    f.summary
                ));
            }
            for (prefix, words) in tree {
                let condition = format!("__hoshi_context \"{prefix}\"");
                out.push_str(&format!(
                    "complete -c hoshi -n '{condition}' -a '{}'\n",
                    words.into_iter().collect::<Vec<_>>().join(" ")
                ));
            }
            Ok(out)
        }
        _ => Err(format!(
            "Unknown completion shell: {shell}\nUse bash, zsh, or fish.\n"
        )),
    }
}

/// The Commands help page uses runnable examples from the same CLI catalog.
pub(crate) fn command_lines(hub: &str) -> Vec<String> {
    let names = match hub {
        "colony" => ["state", "build", "research"],
        "fleet" => ["vehicle list", "mission windows", "fleet send"],
        "trade" => ["market region", "finance", "kpis"],
        "empire" => ["staff", "law propose", "incidents"],
        "records" => ["log", "scenario compare", "prestige"],
        _ => ["state", "advise", "goals"],
    };
    let mut lines = vec!["Keys   [Commands]".into(), String::new()];
    for name in names {
        if let Some(t) = topics().into_iter().find(|t| t.name == name) {
            lines.push(t.summary.into());
            lines.push(format!("  hoshi {}", t.example));
            lines.push(String::new());
        }
    }
    lines.push("More: hoshi help <topic>  docs/commands.md".into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn argv(words: &[&str]) -> Vec<String> {
        std::iter::once("hoshi")
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn help_aliases_topics_and_unknown_input() {
        let expected = invocation(&argv(&["help"])).unwrap();
        assert_eq!(expected.1, 0);
        assert_eq!(invocation(&argv(&["--help"])), Some(expected.clone()));
        assert_eq!(invocation(&argv(&["-h"])), Some(expected));
        for topic in ["state", "build", "serve"] {
            let detailed = invocation(&argv(&["help", topic])).unwrap();
            assert_eq!(detailed.1, 0);
            assert!(detailed.0.contains("Usage: hoshi"));
            assert!(detailed.0.contains("Example:"));
            assert!(detailed.0.contains("Environment:"));
            assert_eq!(invocation(&argv(&[topic, "--help"])), Some(detailed));
        }
        assert_eq!(invocation(&argv(&["help", "missing"])).unwrap().1, 2);
        assert_eq!(
            invocation(&argv(&["help", "--completions", "unknown"]))
                .unwrap()
                .1,
            2
        );
        assert!(invocation(&argv(&["state"])).is_none());
    }

    #[test]
    fn catalog_and_generated_references_stay_complete() {
        let all = topics();
        let mut seen = BTreeSet::new();
        for t in &all {
            assert!(seen.insert(t.name), "duplicate topic {}", t.name);
            assert!(!t.summary.trim().is_empty(), "{} lacks summary", t.name);
            assert!(!t.example.trim().is_empty(), "{} lacks example", t.name);
            assert!(!t.usage.trim().is_empty(), "{} lacks usage", t.name);
        }
        // Top-level routing is intentionally hand-written. Audit its command literals
        // independently so a new entry point cannot silently miss all help exports.
        let source = include_str!("../main.rs");
        let main = source.split("#[derive(Debug)]").next().unwrap();
        for literal in main.split("Some(\"").skip(1) {
            let name = literal.split('"').next().unwrap();
            assert!(seen.contains(name), "entry point {name} lacks catalog help");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (path, expected) in [
            ("docs/commands.md", markdown()),
            ("docs/generated/hoshi.1", man()),
            ("docs/generated/hoshi.bash", completions("bash").unwrap()),
            ("docs/generated/hoshi.zsh", completions("zsh").unwrap()),
            ("docs/generated/hoshi.fish", completions("fish").unwrap()),
        ] {
            assert_eq!(
                std::fs::read_to_string(root.join(path)).unwrap(),
                expected,
                "{path} is stale; run scripts/generate-help.sh"
            );
        }
    }

    #[test]
    fn topic_help_golden() {
        assert_eq!(
            topic_help("state").unwrap(),
            include_str!("../../tests/fixtures/help/state.txt")
        );
        assert_eq!(
            topic_help("serve").unwrap(),
            include_str!("../../tests/fixtures/help/serve.txt")
        );
        assert_eq!(
            topic_help("fleet send").unwrap(),
            include_str!("../../tests/fixtures/help/fleet-send.txt")
        );
    }
}
