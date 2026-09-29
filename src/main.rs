//! `wikirs`: one binary, every Interface a subcommand (ADR 0001). Thin: resolve
//! the Wiki and dispatch.

use std::process::ExitCode;

use clap::Parser;
use wikirs_cli::{Cli, Top, print_outcome};
use wikirs_core::{Command, Discovery, Error, Wiki, config_base, resolve_root};

fn open_wiki(flag: Option<&str>, discovery: Discovery) -> Result<Wiki, Error> {
    let env = std::env::var("WIKIRS_WIKI").ok();
    let cwd = std::env::current_dir().map_err(|e| Error::io(None, &e))?;
    Wiki::open(resolve_root(
        flag,
        env.as_deref(),
        &cwd,
        discovery,
        &config_base(),
    )?)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Top::Op(command) => {
            // `init` creates the `.wikirs/` that walking up looks for: it targets the cwd.
            let discovery = if matches!(command, Command::Init(_)) {
                Discovery::Init
            } else {
                Discovery::Cli
            };
            let wiki = open_wiki(cli.wiki.as_deref(), discovery);
            wikirs_cli::run_operation(command, wiki, cli.json)
        }
        Top::Catalogue => {
            println!(
                "{}",
                serde_json::to_string_pretty(&wikirs_core::catalogue()).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        // MCP clients start us with an unpredictable cwd: never walk up (wiki-selection.md).
        Top::Mcp => match open_wiki(cli.wiki.as_deref(), Discovery::Mcp) {
            Err(err) => print_outcome(&Err(err), cli.json),
            Ok(wiki) => run_mcp(wiki),
        },
    }
}

#[cfg(feature = "mcp")]
fn run_mcp(wiki: Wiki) -> ExitCode {
    match wikirs_mcp::run_stdio(wiki) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error[io]: {err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(feature = "mcp"))]
fn run_mcp(_wiki: Wiki) -> ExitCode {
    eprintln!("error: `mcp` is not built into this binary (rebuild with the `mcp` feature)");
    ExitCode::from(2)
}
