//! `cargo xbin` command-line surface.

use clap::{Parser, Subcommand};

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
        cargo xbin sync     # phase 1: collect metadata, merge, validate and allocate\n  \
        cargo xbin build    # sync, then build every application of the project"
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
    Sync,
    /// Run `sync`, then build every application of the project.
    Build,
}
