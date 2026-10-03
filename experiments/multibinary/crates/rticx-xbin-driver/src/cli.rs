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
        cargo xbin build              # sync, then build every application of the project\n  \
        cargo xbin build --verify-elf # ... and check every linked binary against the pools\n  \
        cargo xbin verify             # check already-built binaries (plain cargo build workflow)"
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
    Build(BuildArgs),
    /// Check the already-built applications' linked binaries against the
    /// synced pools (M6.9-T7): run `cargo xbin sync`, build with plain Cargo,
    /// then this.
    Verify(VerifyArgs),
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

/// Arguments of `cargo xbin build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Verify every linked binary against the distro pools after building
    /// (M6.9-T7). Off by default so the check is opt-in.
    #[arg(long)]
    pub verify_elf: bool,
}

/// Arguments of `cargo xbin verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Check the release binaries instead of the debug ones.
    #[arg(long)]
    pub release: bool,
}
