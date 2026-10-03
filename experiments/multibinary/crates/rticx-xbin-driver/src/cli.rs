//! `cargo xbin` command-line surface.

use clap::{Args, Parser, Subcommand};

/// Multi-binary / heterogeneous multi-core driver for RTICX.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-xbin",
    bin_name = "cargo xbin",
    version,
    about = "Multi-binary / heterogeneous multi-core driver for RTICX",
    subcommand_required = true,
    arg_required_else_help = true,
    after_help = "Examples:\n  \
        cargo xbin sync               # phase 1: collect metadata, merge, validate and allocate\n  \
        cargo xbin sync --html        # ... and render target/rticx-xbin/system.html\n  \
        cargo xbin sync --html-open   # ... render the HTML and open it in a browser\n  \
        cargo xbin build              # sync, then build every application of the project"
)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// `cargo xbin` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Phase 1: parse `rticx.toml` and `ipc-types.toml`, collect the
    /// per-application metadata, merge and validate the system view, allocate
    /// FIFO addresses and write `target/rticx-xbin/system.json`.
    Sync(SyncArgs),
    /// Run `sync`, then build every application of the project.
    Build,
}

/// Arguments of `cargo xbin sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Also render a self-contained HTML visualization of the system view to
    /// `target/rticx-xbin/system.html`.
    #[arg(long)]
    pub html: bool,

    /// Render the HTML visualization (implies `--html`) and open it in the
    /// default browser. Set `RTICX_XBIN_OPENER` to override the opener.
    #[arg(long)]
    pub html_open: bool,
}
