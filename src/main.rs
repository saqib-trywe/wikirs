//! `wikirs`: one binary, every Interface a subcommand (ADR 0001). Thin: resolve
//! the Wiki and dispatch.

use std::process::ExitCode;

use wikirs_cli::{ConfigCommand, Output, ServeArgs, Top, print_error};
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
    let invocation = wikirs_cli::parse();
    let (flag, json) = (invocation.wiki.as_deref(), invocation.json);
    let command = match invocation.command {
        Ok(command) => command,
        Err(err) => return print_error(&err, json),
    };
    match command {
        Top::Op(command) => {
            // `init` creates the `.wikirs/` that walking up looks for: it targets the cwd.
            let discovery = if matches!(command, Command::Init(_)) {
                Discovery::Init
            } else {
                Discovery::Cli
            };
            let output = Output {
                json,
                fail_on_diagnostics: invocation.fail_on_diagnostics,
            };
            wikirs_cli::run_operation(command, open_wiki(flag, discovery), output)
        }
        Top::Config(ConfigCommand::Adopt { from, dry_run }) => wikirs_cli::run_adopt(
            open_wiki(flag, Discovery::Cli),
            from.as_deref(),
            dry_run,
            json,
        ),
        Top::Catalogue => {
            println!(
                "{}",
                serde_json::to_string_pretty(&wikirs_core::catalogue()).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Top::Serve(args) => match open_wiki(flag, Discovery::Cli) {
            Err(err) => print_error(&err, json),
            Ok(wiki) => run_serve(&wiki, &args),
        },
        Top::Tui { page } => match open_wiki(flag, Discovery::Cli) {
            Err(err) => print_error(&err, json),
            Ok(wiki) => run_tui(wiki, page.as_deref()),
        },
        // MCP clients start us with an unpredictable cwd: never walk up (wiki-selection.md).
        Top::Mcp { read_only } => match open_wiki(flag, Discovery::Mcp) {
            Err(err) => print_error(&err, json),
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

#[cfg(feature = "tui")]
fn run_tui(wiki: Wiki, page: Option<&str>) -> ExitCode {
    match wikirs_tui::run(wiki, page) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error[io]: {err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(feature = "tui"))]
fn run_tui(_wiki: Wiki, _page: Option<&str>) -> ExitCode {
    eprintln!("error: `tui` is not built into this binary (rebuild with the `tui` feature)");
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
