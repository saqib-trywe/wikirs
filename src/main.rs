//! `wikirs`: one binary, every Interface a subcommand (ADR 0001). Thin: resolve
//! the Wiki and dispatch.

use std::process::ExitCode;

use clap::Parser;
use wikirs_cli::{Cli, ServeArgs, Top, print_outcome};
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
        Top::Serve(args) => match open_wiki(cli.wiki.as_deref(), Discovery::Cli) {
            Err(err) => print_outcome(&Err(err), cli.json),
            Ok(wiki) => run_serve(&wiki, &args),
        },
        // MCP clients start us with an unpredictable cwd: never walk up (wiki-selection.md).
        Top::Mcp { read_only } => match open_wiki(cli.wiki.as_deref(), Discovery::Mcp) {
            Err(err) => print_outcome(&Err(err), cli.json),
            Ok(wiki) => run_mcp(wiki, read_only),
        },
    }
}

#[cfg(feature = "mcp")]
fn run_mcp(wiki: Wiki, read_only: bool) -> ExitCode {
    match wikirs_mcp::run_stdio(wiki, read_only) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error[io]: {err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(feature = "mcp"))]
fn run_mcp(_wiki: Wiki, _read_only: bool) -> ExitCode {
    eprintln!("error: `mcp` is not built into this binary (rebuild with the `mcp` feature)");
    ExitCode::from(2)
}

#[cfg(feature = "http")]
fn run_serve(wiki: &Wiki, args: &ServeArgs) -> ExitCode {
    let outcome = if args.print_token {
        wikirs_http::token::get_or_create(wiki).map(|(token, _)| println!("{token}"))
    } else if args.rotate_token {
        wikirs_http::token::rotate(wiki).map(|token| println!("{token}"))
    } else {
        let options = wikirs_http::ServeOptions {
            bind: args.bind.clone(),
            port: args.port,
            allow_remote: args.allow_remote,
            read_only: args.read_only,
        };
        wikirs_http::serve(wiki, &options)
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(feature = "http"))]
fn run_serve(_wiki: &Wiki, _args: &ServeArgs) -> ExitCode {
    eprintln!("error: `serve` is not built into this binary (rebuild with the `http` feature)");
    ExitCode::from(2)
}
