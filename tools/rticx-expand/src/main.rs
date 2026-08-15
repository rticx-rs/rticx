//! `cargo rticx-expand` — expands RTICX applications into plain Rust for GDB
//! step-debugging, code inspection, and security vetting.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use rticx_expand::flow;

#[derive(Parser)]
#[command(
    name = "rticx-expand",
    version,
    about = "Expand RTICX applications into plain Rust for GDB debugging, inspection, and security vetting",
    after_help = "Examples:\n  \
        cargo rticx-expand --example hello_rtic --features swtasks          # prints the complete expanded file to stdout\n  \
        cargo rticx-expand --example hello_rtic --features swtasks > expanded.rs\n  \
        cargo rticx-expand --example hello_rtic --features swtasks --merge  # replaces the app module in the source file\n  \
        cargo rticx-expand --example hello_rtic --features swtasks --expand-passes target/passes\n  \
        cargo rticx-expand restore"
)]
struct Cli {
    /// Expand options. When no subcommand is given, `expand` runs with these.
    #[command(flatten)]
    expand: ExpandArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Expand the application and print the result to stdout (like
    /// `cargo expand`). Pass --merge to splice it into the source file.
    Expand(ExpandArgs),
    /// Restore the original source from the `.old` backup.
    Restore(RestoreArgs),
}

#[derive(Args)]
struct ExpandArgs {
    /// Path to Cargo.toml of the application crate.
    #[arg(long, default_value = "Cargo.toml")]
    manifest_path: PathBuf,

    /// Build the specified binary.
    #[arg(long, conflicts_with = "example")]
    bin: Option<String>,

    /// Build the specified example.
    #[arg(long, conflicts_with = "bin")]
    example: Option<String>,

    /// File containing the `#[<distro>::app]` module. Auto-detected from the
    /// binary/example target when omitted.
    #[arg(long)]
    file: Option<PathBuf>,

    /// Build for the given target triple.
    #[arg(long)]
    target: Option<String>,

    /// Space or comma separated list of features to activate.
    #[arg(long)]
    features: Option<String>,

    /// Activate all available features.
    #[arg(long, conflicts_with = "features")]
    all_features: bool,

    /// Do not activate the default features.
    #[arg(long)]
    no_default_features: bool,

    /// Directory where the module state is snapshotted after every compilation
    /// pass and after the core pass (`00_original.rs`, `01_<Pass>.rs`, …,
    /// `NN_core.rs`) — diff consecutive files to see what each stage changed.
    #[arg(long, value_name = "DIR")]
    expand_passes: Option<PathBuf>,

    /// Directory for expansion files (default: <target>/rticx-expand).
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Replace the `#[<distro>::app]` module in the source file with the
    /// expansion (the original is kept as `<file>.old`). Without this flag,
    /// the expansion is printed to stdout.
    #[arg(long)]
    merge: bool,

    /// Overwrite an existing `.old` backup.
    #[arg(long)]
    force: bool,

    /// Extra arguments passed to `cargo check`.
    #[arg(last = true)]
    cargo_args: Vec<String>,

    /// Verbose output.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct RestoreArgs {
    /// Path to Cargo.toml of the application crate.
    #[arg(long, default_value = "Cargo.toml")]
    manifest_path: PathBuf,

    /// Merged file to restore (auto-detected from `.rs.old` backups when
    /// omitted).
    #[arg(long)]
    file: Option<PathBuf>,

    /// Also delete the expansion directory.
    #[arg(long)]
    remove_expansions: bool,

    /// Verbose output.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> ExitCode {
    // When invoked as `cargo rticx-expand …`, cargo passes the subcommand
    // name as the first argument; strip it.
    let mut argv: Vec<_> = std::env::args_os().collect();
    if argv.get(1).and_then(|a| a.to_str()) == Some("rticx-expand") {
        argv.remove(1);
    }
    let cli = Cli::parse_from(argv);

    let args = match cli.command {
        Some(Command::Restore(args)) => return run_restore(&args),
        Some(Command::Expand(args)) => args,
        None => cli.expand,
    };

    let result = flow::expand(&flow::ExpandOptions {
        manifest_path: &args.manifest_path,
        bin: args.bin.as_deref(),
        example: args.example.as_deref(),
        file: args.file.as_deref(),
        target: args.target.as_deref(),
        features: args.features.as_deref(),
        all_features: args.all_features,
        no_default_features: args.no_default_features,
        expand_passes: args.expand_passes.as_deref(),
        output_dir: args.output_dir.as_deref(),
        merge: args.merge,
        force: args.force,
        cargo_args: &args.cargo_args,
        verbose: args.verbose,
    });

    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("[rticx-expand] error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_restore(args: &RestoreArgs) -> ExitCode {
    let result = flow::restore(&flow::RestoreOptions {
        manifest_path: &args.manifest_path,
        file: args.file.as_deref(),
        remove_expansions: args.remove_expansions,
        verbose: args.verbose,
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("[rticx-expand] error: {err}");
            ExitCode::FAILURE
        }
    }
}
