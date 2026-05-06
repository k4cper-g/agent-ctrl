//! `agent-ctrl` CLI entrypoint.

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod commands;
mod doctor;
mod info;

/// Cross-platform computer-use framework for AI agents.
#[derive(Debug, Parser)]
#[command(name = "agent-ctrl", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: commands::Command,
}

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if args_request_json() {
                commands::print_parse_error_json(&error);
                std::process::exit(error.exit_code());
            }
            error.exit();
        }
    };
    let json_errors = cli.command.wants_json_output();
    if let Err(error) = cli.command.run().await {
        if json_errors {
            commands::print_error_json(&error);
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}

fn args_request_json() -> bool {
    std::env::args_os().any(|arg| arg == "--json")
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_env("AGENT_CTRL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
