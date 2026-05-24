mod app;
mod git;
mod model;
mod patch;
mod syntax;
mod theme;
mod ui;

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;

#[derive(Debug, Parser)]
#[command(name = "chunk")]
#[command(about = "Minimal terminal diff review")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Review the current Git working tree diff.
    Diff,
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Diff) {
        Command::Diff => app::run(git::load_worktree_diff()?),
    }
}
