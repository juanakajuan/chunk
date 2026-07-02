//! Command-line entry point for `chunk`.
//!
//! This module keeps startup small: parse CLI arguments, load the requested
//! diff source, then hand the resulting changeset to the terminal app.

mod app;
mod ask_ai;
mod clipboard;
mod config;
mod custom_command;
mod diff_json;
mod diff_render;
mod editor;
mod git;
mod keybind;
mod model;
mod patch;
mod process;
mod review_source;
mod rows;
mod runtime;
mod scroll_text;
mod search;
mod selection;
mod syntax;
mod theme;
mod ui;
mod viewport;

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
    Diff {
        /// Print a JSON snapshot instead of opening the terminal UI.
        #[arg(long)]
        json: bool,
    },
    /// Review the current branch against a base branch, like a pull request.
    Pr {
        /// Base branch/ref. Defaults to origin/HEAD, then main, then master.
        base: Option<String>,
    },
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let review = match cli.command.unwrap_or(Command::Diff { json: false }) {
        Command::Diff { json: true } => return diff_json::print(&git::load_worktree_diff()?),
        Command::Diff { json: false } => review_source::ReviewSource::load_worktree()?,
        Command::Pr { base } => review_source::ReviewSource::load_pull_request(base.as_deref())?,
    };

    runtime::run(app::App::with_config(review, config::load()?))
}
