//! `agent-ctrl` CLI entrypoint.

#![forbid(unsafe_code)]

use anyhow::Result;
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
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    cli.command.run().await
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_env("AGENT_CTRL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
