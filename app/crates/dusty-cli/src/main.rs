//! `dusty`: Dusty in a terminal. Argument parsing and dispatch only; each command lives in
//! [`commands`], text output in [`render`], JSON output in [`json`], and every rule in `dusty-core`.

mod commands;
mod json;
mod render;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dusty", version, about = "Reclaim disk space on macOS: build artifacts, dev caches, Xcode, Android, Docker.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    #[command(flatten)]
    opts: Opts,
}

/// Options shared by every command.
#[derive(clap::Args, Clone)]
pub struct Opts {
    /// Folders to scan for project build artifacts (default: Documents, Developer, Projects, agent worktrees…)
    #[arg(short, long, num_args = 1.., global = true)]
    pub roots: Vec<PathBuf>,
    /// Show what would be removed, remove nothing
    #[arg(short = 'n', long, global = true)]
    pub dry_run: bool,
    /// No prompts: clean everything selected by default
    #[arg(short, long, global = true)]
    pub yes: bool,
    /// Also pre-select opt-in items (re-downloads, Docker images…)
    #[arg(short, long, global = true)]
    pub all: bool,
    /// Skip project artifacts modified in the last N days
    #[arg(short, long, value_name = "DAYS", default_value_t = 0, global = true)]
    pub keep_recent: u64,
    /// Hide items smaller than N MB
    #[arg(short, long, value_name = "MB", default_value_t = 1, global = true)]
    pub min_size: u64,
    /// Machine-readable output (scan, history, large)
    #[arg(long, global = true)]
    pub json: bool,
    /// Move to the Trash instead of deleting (undo with `dusty put-back <id>`)
    #[arg(short, long, global = true)]
    pub trash: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Analyse disk usage, delete nothing
    Scan,
    /// Pick and remove (the default)
    Clean,
    /// Past cleans, newest first
    History,
    /// Move a clean's trashed items back where they were
    PutBack { id: String },
    /// Review big and long-unused files (nothing preselected, picks go to the Trash)
    Large(LargeArgs),
    /// The weekly safe clean
    Schedule {
        #[arg(value_enum, default_value_t = ScheduleState::Status)]
        state: ScheduleState,
    },
}

#[derive(clap::Args)]
pub struct LargeArgs {
    /// "Large" means at least N MB
    #[arg(long, value_name = "MB", default_value_t = 200)]
    pub over: u64,
    /// "Old" means not opened for N days
    #[arg(long, value_name = "DAYS", default_value_t = 180)]
    pub unused: u64,
    /// Only files that are both large and old
    #[arg(long)]
    pub both: bool,
    /// Ignore age
    #[arg(long)]
    pub size_only: bool,
}

#[derive(ValueEnum, Clone, Copy)]
pub enum ScheduleState {
    On,
    Off,
    Status,
}

fn main() {
    dusty_core::fix_path();
    // launchd runs `dusty --scheduled-clean --policy=…` (the flag the app uses too)
    if std::env::args().any(|a| a == dusty_core::schedule::SCHEDULED_CLEAN_FLAG) {
        std::process::exit(dusty_core::schedule::run_scheduled_clean_main(std::env::args()));
    }
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Cmd::Clean) {
        Cmd::Scan => commands::scan(&cli.opts),
        Cmd::Clean => commands::clean(&cli.opts),
        Cmd::History => commands::history(&cli.opts),
        Cmd::PutBack { id } => commands::put_back(&id),
        Cmd::Large(args) => commands::large(&cli.opts, &args),
        Cmd::Schedule { state } => commands::schedule(state),
    };
    if let Err(e) = result {
        render::error(&e);
        std::process::exit(1);
    }
}
