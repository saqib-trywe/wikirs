//! The CLI Interface: one subcommand per Operation, generated from the registry
//! (docs/spec/interfaces.md), with the output and exit codes of docs/spec/errors.md.
//!
//! Every Operation subcommand also takes `--input <json | ->`, its whole Input as
//! JSON. Human output goes through [`render`]; `--json` prints the envelope.

pub mod render;

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::Path,
    process::ExitCode,
};

use clap::{
    Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, builder::Resettable,
};
use serde_json::{Value, json};
use wikirs_core::{
    Error, ErrorKind, Wiki,
    config::{adopt, adoptable},
    watch::{Watch, WatchInput},
};

#[derive(Debug, Parser)]
#[command(name = "wikirs", version, about = "A local markdown wiki")]
pub struct Cli {
    /// Wiki root or named Wiki (else `WIKIRS_WIKI`, else walk up to `.wikirs/`).
    #[arg(long, global = true)]
    pub wiki: Option<String>,
    /// Print the `{ result, warnings }` envelope (or the error) as JSON on stdout.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Top,
}

#[derive(Debug, Subcommand)]
pub enum Top {
    #[command(flatten)]
    Op(wikirs_core::Command),
    /// Serve the Wiki over MCP on stdio.
    Mcp {
        /// List only queries and `rebuild_index` as tools; resources stay available.
        #[arg(long)]
        read_only: bool,
    },
    /// Serve the Wiki over HTTP (`/ops`) and MCP Streamable HTTP (`/mcp`).
    Serve(ServeArgs),
    /// Open the terminal UI.
    Tui {
        /// Page Path to open first (else the first Page in the tree).
        page: Option<String>,
    },
    /// Machine settings that aren't Operations.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Print the Operation catalogue as JSON (dev tool).
    Catalogue,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Take over the machine settings of this Wiki from before it moved.
    Adopt {
        /// The Wiki's old root, when several moved Wikis' settings could be adopted.
        #[arg(long)]
        from: Option<String>,
        /// Show the change without making it.
        #[arg(long)]
        dry_run: bool,
    },
}

/// `wikirs serve`: each flag overrides its `[serve]` machine setting.
#[derive(Debug, clap::Args)]
#[allow(clippy::struct_excessive_bools)] // independent CLI flags
pub struct ServeArgs {
    /// Address to listen on (default: `127.0.0.1` and `[::1]`).
    #[arg(long)]
    pub bind: Option<String>,
    /// Port (default 4747; 0 picks a free one).
    #[arg(long)]
    pub port: Option<u16>,
    /// Allow a non-loopback --bind. Token auth is then always on.
    #[arg(long)]
    pub allow_remote: bool,
    /// No mutations over HTTP or MCP.
    #[arg(long)]
    pub read_only: bool,
    /// Print this Wiki's token (generating it if needed) and exit.
    #[arg(long, conflicts_with = "rotate_token")]
    pub print_token: bool,
    /// Replace this Wiki's token, print the new one, and exit.
    #[arg(long)]
    pub rotate_token: bool,
}

const INPUT: &str = "input";
const FAIL_ON_DIAGNOSTICS: &str = "fail_on_diagnostics";
/// Subcommands that aren't Operations.
const OURS: [&str; 6] = ["mcp", "serve", "tui", "config", "catalogue", "help"];

/// A parsed command line. `command` is an error when `--input` doesn't hold a
/// valid Input; it is printed like an Operation's error.
#[derive(Debug)]
pub struct Invocation {
    pub wiki: Option<String>,
    pub json: bool,
    /// `check --fail-on-diagnostics`: exit 7 if there are any.
    pub fail_on_diagnostics: bool,
    pub command: Result<Top, Error>,
}

/// Subcommand names the CLI exposes for Operations (the parity test compares
/// these with the registry).
#[must_use]
pub fn operation_subcommands() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .filter(|n| !OURS.contains(&n.as_str()))
        .collect()
}

/// The full clap command: the derived one plus `--input` on every Operation
/// and `--fail-on-diagnostics` on `check`. `relaxed` (when `--input` is on the
/// line) makes every other argument optional and in conflict with `--input`,
/// since the JSON carries them.
#[must_use]
pub fn command(relaxed: bool) -> clap::Command {
    let mut cmd = Cli::command();
    for name in operation_subcommands() {
        cmd = cmd.mut_subcommand(&name, |mut sub| {
            let others: Vec<clap::Id> = sub.get_arguments().map(|a| a.get_id().clone()).collect();
            let groups: Vec<clap::Id> = sub.get_groups().map(|g| g.get_id().clone()).collect();
            let mut input = Arg::new(INPUT)
                .long("input")
                .value_name("JSON|-")
                .allow_hyphen_values(true)
                .help(
                    "The whole Input as JSON (`-` reads it from stdin), instead of the arguments",
                );
            if relaxed {
                input = input.conflicts_with_all(&others);
                for id in &others {
                    sub = sub.mut_arg(id, |a| {
                        a.required(false)
                            .required_unless_present(Resettable::<clap::Id>::Reset)
                    });
                }
                for id in &groups {
                    sub = sub.mut_group(id, |g| g.required(false));
                }
            }
            sub = sub.arg(input);
            if name == "check" {
                sub = sub.arg(
                    Arg::new(FAIL_ON_DIAGNOSTICS)
                        .long("fail-on-diagnostics")
                        .action(ArgAction::SetTrue)
                        .help("Exit with code 7 if there are any diagnostics"),
                );
            }
            sub
        });
    }
    cmd
}

/// Parses the process's command line, exiting on a usage error (code 2).
#[must_use]
pub fn parse() -> Invocation {
    parse_from(std::env::args_os())
}

/// [`parse`] over given arguments (`args[0]` is the binary).
#[must_use]
pub fn parse_from(args: impl IntoIterator<Item = impl Into<OsString>>) -> Invocation {
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let mentions_input = args
        .iter()
        .any(|a| a == "--input" || a.to_string_lossy().starts_with("--input="));
    let matches = command(mentions_input).get_matches_from(&args);
    let input = matches.subcommand().and_then(|(name, sub)| {
        // Subcommands that aren't Operations have no `--input`.
        let raw = sub.try_get_one::<String>(INPUT).ok().flatten()?;
        Some((name.replace('-', "_"), raw))
    });
    let command = if let Some((name, raw)) = input {
        read_input(raw)
            .and_then(|input| wikirs_core::Command::from_json(&name, input))
            .map(Top::Op)
    } else {
        // Also when `--input` was only an argument's value: a missing
        // argument then fails here, as a usage error.
        Ok(Cli::from_arg_matches(&matches)
            .unwrap_or_else(|e| e.format(&mut command(false)).exit())
            .command)
    };
    let fail_on_diagnostics = matches
        .subcommand_matches("check")
        .is_some_and(|sub| sub.get_flag(FAIL_ON_DIAGNOSTICS));
    Invocation {
        wiki: matches.get_one::<String>("wiki").cloned(),
        json: matches.get_flag("json"),
        fail_on_diagnostics,
        command,
    }
}

fn read_input(raw: &str) -> Result<Value, Error> {
    let text = if raw == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| Error::io(None, &e))?;
        text
    } else {
        raw.to_string()
    };
    serde_json::from_str(&text)
        .map_err(|e| Error::invalid_input(Some("input"), format!("not JSON: {e}")))
}

/// How a run should print and exit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Output {
    pub json: bool,
    pub fail_on_diagnostics: bool,
}

/// Runs one Operation subcommand and prints its outcome.
#[must_use]
pub fn run_operation(
    command: wikirs_core::Command,
    wiki: Result<Wiki, Error>,
    output: Output,
) -> ExitCode {
    if let wikirs_core::Command::Watch(input) = command {
        return run_watch(input, wiki, output.json);
    }
    let op = command.name();
    let wiki = match wiki {
        Ok(wiki) => wiki,
        Err(err) => return print_error(&err, output.json),
    };
    if !output.json {
        adoption_hint(&wiki);
    }
    let outcome = command.run(&wiki);
    let code = print_outcome(op, &outcome, output.json, wiki.root());
    let diagnostics = outcome
        .as_ref()
        .ok()
        .and_then(|envelope| envelope["result"]["diagnostics"].as_array())
        .is_some_and(|d| !d.is_empty());
    if output.fail_on_diagnostics && diagnostics {
        return ExitCode::from(7);
    }
    code
}

/// `wikirs config adopt`.
#[must_use]
pub fn run_adopt(
    wiki: Result<Wiki, Error>,
    from: Option<&str>,
    dry_run: bool,
    json: bool,
) -> ExitCode {
    let outcome = wiki.and_then(|wiki| {
        let adopted = adopt(&wiki, from, dry_run)?;
        let human = if json {
            Vec::new()
        } else {
            let verb = if dry_run { "would adopt" } else { "adopted" };
            let plan = if dry_run {
                render::diff(&adopted.plan, wiki.root())
            } else {
                render::summary(&adopted.plan)
            };
            format!("{verb} the settings of {}\n{plan}", adopted.from.root).into_bytes()
        };
        let warnings = adopted.plan.warnings.clone();
        let result = serde_json::to_value(&adopted).map_err(|e| Error::internal(e.to_string()))?;
        Ok((json!({ "result": result, "warnings": warnings }), human))
    });
    match outcome {
        Ok((envelope, human)) => {
            if json {
                println!("{envelope}");
            } else {
                print_warnings(&envelope);
                write_stdout(&human);
            }
            ExitCode::SUCCESS
        }
        Err(err) => print_error(&err, json),
    }
}

/// Points at `config adopt` when this Wiki has no machine settings but a moved one left some.
fn adoption_hint(wiki: &Wiki) {
    if let Some(found) = adoptable(wiki).first() {
        eprintln!(
            "hint: settings of a Wiki that moved from {} can be adopted: `wikirs config adopt`",
            found.root
        );
    }
}

/// `watch`: one JSON event per line on stdout, until stdout closes or the
/// process is stopped.
fn run_watch(input: WatchInput, wiki: Result<Wiki, Error>, json: bool) -> ExitCode {
    // The handle owns the watcher and the subscription: it must outlive the loop.
    let (events, _wiki) = match wiki.and_then(|wiki| Ok((Watch::subscribe(&wiki, input)?, wiki))) {
        Ok(subscribed) => subscribed,
        Err(err) => return print_error(&err, json),
    };
    let mut out = std::io::stdout().lock();
    for event in events {
        let line = serde_json::to_string(&event).unwrap_or_default();
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// Prints an Operation's outcome: the envelope with `--json`, else warnings on
/// stderr and the rendered result on stdout.
#[must_use]
pub fn print_outcome(
    op: &str,
    outcome: &Result<Value, Error>,
    json: bool,
    root: &Path,
) -> ExitCode {
    match outcome {
        Ok(envelope) if json => {
            println!("{envelope}");
            ExitCode::SUCCESS
        }
        Ok(envelope) => {
            print_warnings(envelope);
            write_stdout(&render::render(op, &envelope["result"], root));
            ExitCode::SUCCESS
        }
        Err(err) => print_error(err, json),
    }
}

/// Prints an error (stdout with `--json`, else stderr with a hint) and returns its exit code.
#[must_use]
pub fn print_error(err: &Error, json: bool) -> ExitCode {
    if json {
        println!("{}", err.to_json());
    } else {
        eprintln!("{err}");
        if let Some(hint) = hint(err) {
            eprintln!("hint: {hint}");
        }
    }
    ExitCode::from(exit_code(err.kind))
}

fn hint(err: &Error) -> Option<&'static str> {
    match (err.kind, err.details["reason"].as_str()) {
        (ErrorKind::Conflict, Some("lock_timeout")) => {
            Some("another wikirs process is writing to this Wiki; try again")
        }
        (ErrorKind::Conflict, _) => {
            Some("read the Page again, then retry with its new version as --base-version")
        }
        (ErrorKind::NoMatch | ErrorKind::AmbiguousMatch, _) => {
            Some("each edit's `old` must occur exactly once; include more surrounding text")
        }
        _ => None,
    }
}

fn print_warnings(envelope: &Value) {
    for warning in envelope["warnings"].as_array().into_iter().flatten() {
        eprintln!(
            "warning: {}",
            warning["message"].as_str().unwrap_or_default()
        );
    }
}

fn write_stdout(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    // A closed pipe (`wikirs … | head`) isn't an error worth reporting.
    let _ = out.write_all(bytes).and_then(|()| out.flush());
}

/// CLI exit codes (errors.md).
#[must_use]
pub fn exit_code(kind: ErrorKind) -> u8 {
    match kind {
        ErrorKind::Io | ErrorKind::Internal => 1,
        ErrorKind::InvalidInput | ErrorKind::InvalidPath => 2,
        ErrorKind::NotFound => 3,
        ErrorKind::AlreadyExists | ErrorKind::CaseConflict => 4,
        ErrorKind::Conflict => 5,
        ErrorKind::NoMatch | ErrorKind::AmbiguousMatch => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(args: &[&str]) -> Invocation {
        parse_from(std::iter::once("wikirs").chain(args.iter().copied()))
    }

    #[test]
    fn input_json_stands_in_for_the_arguments() {
        let inv = op(&[
            "move-page",
            "--input",
            r#"{"from":"a","to":"b","dry_run":true}"#,
        ]);
        let Ok(Top::Op(command)) = inv.command else {
            panic!("{inv:?}")
        };
        assert_eq!(command.name(), "move_page");
        let shown = format!("{command:?}");
        assert!(
            shown.contains("dry_run: true") && shown.contains("\"b\""),
            "{shown}"
        );

        let inv = op(&["--json", "add-attachment", "--input={\"page\":\"p\"}"]);
        assert!(inv.json);
        let err = inv.command.unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput, "{err}");

        let inv = op(&["reorder-page", "--input", r#"{"page":"a","after":"b"}"#]);
        assert!(
            inv.command.is_ok(),
            "required_unless_present is lifted: {inv:?}"
        );

        let err = op(&["list-pages", "--input", "{"]).command.unwrap_err();
        assert_eq!(err.details["field"], "input");
    }

    #[test]
    fn input_as_an_arguments_value_is_just_text() {
        let inv = op(&["write-page", "p", "--content", "--input"]);
        let Ok(Top::Op(wikirs_core::Command::WritePage(input))) = inv.command else {
            panic!("{inv:?}")
        };
        assert_eq!(input.content, "--input");
    }

    #[test]
    fn subcommands_that_are_not_operations_parse_as_before() {
        let inv = op(&["config", "adopt", "--from", "/old"]);
        assert!(
            matches!(&inv.command, Ok(Top::Config(ConfigCommand::Adopt { from: Some(f), dry_run: false })) if f == "/old"),
            "{inv:?}"
        );
        assert!(matches!(op(&["catalogue"]).command, Ok(Top::Catalogue)));
    }

    #[test]
    fn input_conflicts_with_the_arguments_it_replaces() {
        let err = command(true)
            .try_get_matches_from(["wikirs", "move-page", "a", "--input", "{}"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        let ok = command(true).try_get_matches_from([
            "wikirs",
            "check",
            "--fail-on-diagnostics",
            "--input",
            "{}",
        ]);
        assert!(ok.is_ok(), "{ok:?}");
        let err = command(false)
            .try_get_matches_from(["wikirs", "move-page", "a"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
