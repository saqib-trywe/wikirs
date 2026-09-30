//! The CLI Interface: one subcommand per Operation, generated from the registry
//! (docs/spec/interfaces.md), with the output and exit codes of docs/spec/errors.md.
//!
//! Walking skeleton: human output is pretty JSON; the per-type `Render` trait,
//! `--input <json>` and unified-diff Plans come in later slices.

use std::{io::Write, process::ExitCode};

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;
use wikirs_core::{
    Error, ErrorKind, Wiki,
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
    /// Print the Operation catalogue as JSON (dev tool).
    Catalogue,
}

/// Subcommand names the CLI exposes for Operations (the parity test compares
/// these with the registry).
#[must_use]
pub fn operation_subcommands() -> Vec<String> {
    let ours = ["mcp", "catalogue", "help"];
    Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .filter(|n| !ours.contains(&n.as_str()))
        .collect()
}

/// Runs one Operation subcommand and prints its outcome.
#[must_use]
pub fn run_operation(
    command: wikirs_core::Command,
    wiki: Result<Wiki, Error>,
    json: bool,
) -> ExitCode {
    if let wikirs_core::Command::Watch(input) = command {
        return run_watch(input, wiki, json);
    }
    let outcome = wiki.and_then(|wiki| command.run(&wiki));
    print_outcome(&outcome, json)
}

/// `watch`: one JSON event per line on stdout, until stdout closes or the
/// process is stopped.
fn run_watch(input: WatchInput, wiki: Result<Wiki, Error>, json: bool) -> ExitCode {
    // The handle owns the watcher and the subscription: it must outlive the loop.
    let (events, _wiki) = match wiki.and_then(|wiki| Ok((Watch::subscribe(&wiki, input)?, wiki))) {
        Ok(subscribed) => subscribed,
        Err(err) => return print_outcome(&Err(err), json),
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

#[must_use]
pub fn print_outcome(outcome: &Result<Value, Error>, json: bool) -> ExitCode {
    match outcome {
        Ok(envelope) => {
            if json {
                println!("{envelope}");
            } else {
                for warning in envelope["warnings"].as_array().into_iter().flatten() {
                    eprintln!(
                        "warning: {}",
                        warning["message"].as_str().unwrap_or_default()
                    );
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&envelope["result"]).unwrap_or_default()
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                println!("{}", err.to_json());
            } else {
                eprintln!("{err}");
            }
            ExitCode::from(exit_code(err.kind))
        }
    }
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
