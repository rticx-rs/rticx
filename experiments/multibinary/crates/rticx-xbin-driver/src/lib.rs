//! `cargo-xbin` driver for the RTICX multi-binary extension.
//!
//! The driver is the single merger and address allocator of the multi-binary
//! pipeline (see `multibinary-multicore-plan.md` §7 and §8):
//!
//! - `cargo xbin sync`: phase 1 -- collect per-application metadata, merge
//!   and validate the system view (M1-T5), allocate the per-task FIFO
//!   addresses and emit `target/rticx-xbin/system.json` (M1-T6), and
//!   generate/update the `ipc-types` crate (M1-T7);
//! - `cargo xbin build`: `sync`, then build every application.

mod cli;
mod commands;
mod error;
mod project;

use std::ffi::OsString;

use clap::Parser;
use rticx_xbin_proto::FileChange;

pub use cli::{Cli, Command};
pub use commands::{BuildOutcome, IpcTypesOutcome, SyncOutcome, build, sync};
pub use error::DriverError;
pub use project::{
    IDL_MANIFEST, OUTPUT_DIR, PROJECT_MANIFEST, SYSTEM_FILE, find_project_root, output_dir,
};

/// Name under which Cargo invokes this binary (`cargo xbin`).
pub const SUBCOMMAND: &str = "xbin";

/// Runs `cargo xbin` from a raw argument vector and returns the process exit
/// code.
///
/// Help and version requests exit with `0`; usage errors with `2`; runtime
/// failures with `1`.
pub fn main_entry(mut argv: Vec<OsString>) -> i32 {
    strip_subcommand_name(&mut argv);
    match Cli::try_parse_from(argv) {
        Ok(cli) => match run(cli) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("[cargo-xbin] error: {error}");
                1
            }
        },
        Err(error) => {
            let _ = error.print();
            error.exit_code()
        }
    }
}

/// Removes the `xbin` Cargo injects as the first argument when the binary is
/// invoked as `cargo xbin …`, so both spellings parse identically.
fn strip_subcommand_name(argv: &mut Vec<OsString>) {
    if argv.get(1).and_then(|argument| argument.to_str()) == Some(SUBCOMMAND) {
        argv.remove(1);
    }
}

/// Resolves the project root and dispatches on the parsed subcommand.
fn run(cli: Cli) -> Result<(), DriverError> {
    let current_dir =
        std::env::current_dir().map_err(|source| DriverError::CurrentDir { source })?;
    let project_root = find_project_root(&current_dir).unwrap_or(current_dir);
    match cli.command {
        Command::Sync => {
            let outcome = sync(&project_root)?;
            report_ipc_types(&outcome);
            Ok(())
        }
        Command::Build => {
            let outcome = build(&project_root)?;
            report_ipc_types(&outcome.sync);
            Ok(())
        }
    }
}

/// Reports generated `ipc-types` changes (M1-T7) on standard error, like
/// Cargo's own progress output.
///
/// Library callers inspect [`SyncOutcome::ipc_types`] instead; this is pure
/// presentation.
fn report_ipc_types(outcome: &SyncOutcome) {
    let Some(ipc_types) = &outcome.ipc_types else {
        return;
    };
    let dir = ipc_types
        .path
        .strip_prefix(&outcome.project_root)
        .unwrap_or(&ipc_types.path);
    for file in &ipc_types.status.files {
        let path = dir.join(&file.path);
        match file.change {
            FileChange::Created => eprintln!("[cargo-xbin] created {}", path.display()),
            FileChange::Updated => eprintln!("[cargo-xbin] updated {}", path.display()),
            FileChange::Unchanged => {}
        }
    }
    if ipc_types.status.is_noop() {
        eprintln!("[cargo-xbin] {} is up to date", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::strip_subcommand_name;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn strips_the_cargo_subcommand_name() {
        let mut args = argv(&["cargo-xbin", "xbin", "sync"]);
        strip_subcommand_name(&mut args);
        assert_eq!(args, argv(&["cargo-xbin", "sync"]));

        let mut args = argv(&["cargo-xbin", "sync"]);
        strip_subcommand_name(&mut args);
        assert_eq!(args, argv(&["cargo-xbin", "sync"]));

        // A trailing argument spelling `xbin` is not the subcommand name.
        let mut args = argv(&["cargo-xbin", "sync", "xbin"]);
        strip_subcommand_name(&mut args);
        assert_eq!(args, argv(&["cargo-xbin", "sync", "xbin"]));
    }
}
