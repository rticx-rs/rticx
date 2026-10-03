//! `cargo-xbin` driver for the RTICX multi-binary extension.
//!
//! The driver is the single merger and address allocator of the multi-binary
//! pipeline (see `multibinary-multicore-plan.md` §7 and §8):
//!
//! - `cargo xbin sync`: phase 1 -- collect per-application metadata, merge
//!   and validate the system view (M1-T5), allocate the per-task FIFO
//!   addresses and emit `target/rticx-xbin/system.json` (M1-T6), and
//!   generate/update the `ipc-types` crate (M1-T7);
//! - `cargo xbin build`: `sync`, then build every application against the
//!   emitted system view (M4-T1); `--verify-elf` additionally checks every
//!   linked binary against the distro IPC pools (M6.9-T7);
//! - `cargo xbin verify`: check the already-built applications' linked
//!   binaries against the synced pools, for the plain-`cargo build` workflow
//!   (M6.9-T7).
//!
//! `cargo xbin sync --html` additionally renders a self-contained HTML
//! visualization of the system view to `target/rticx-xbin/system.html`
//! (`--html-open` renders and opens it).

mod cli;
mod commands;
mod elf;
mod error;
mod project;
mod visualize;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use clap::Parser;
use rticx_xbin_proto::FileChange;

pub use cli::{BuildArgs, Cli, Command, SyncArgs, VerifyArgs};
pub use commands::{
    AppBuild, BuildOutcome, IpcTypesOutcome, SyncOutcome, VerifyOutcome, build, build_and_verify,
    sync, verify,
};
use commands::{CargoOutput, build_with_output, sync_with_output, verify_with_output};
pub use elf::{ElfVerification, verify_binary};
pub use error::{DriverError, VerifyError};
pub use project::{
    HTML_FILE, IDL_MANIFEST, OUTPUT_DIR, PROJECT_MANIFEST, SYSTEM_FILE, find_project_root,
    output_dir,
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
        Command::Sync(args) => {
            let outcome = sync_with_output(&project_root, CargoOutput::Streamed)?;
            report_ipc_types(&outcome);
            if args.html || args.html_open {
                let path = write_html(&outcome)?;
                let shown = path.strip_prefix(&outcome.project_root).unwrap_or(&path);
                eprintln!("[cargo-xbin] wrote {}", shown.display());
                if args.html_open
                    && let Err(error) = open_in_browser(&path)
                {
                    eprintln!(
                        "[cargo-xbin] warning: failed to open `{}`: {error}",
                        path.display()
                    );
                }
            }
            Ok(())
        }
        Command::Build(args) => {
            let outcome = build_with_output(&project_root, CargoOutput::Streamed, args.verify_elf)?;
            report_ipc_types(&outcome.sync);
            report_verifications(&outcome.verifications);
            Ok(())
        }
        Command::Verify(args) => {
            let outcome = verify_with_output(&project_root, CargoOutput::Streamed, args.release)?;
            report_verifications(&outcome.verifications);
            Ok(())
        }
    }
}

/// Renders and writes the HTML visualization of a completed sync.
///
/// `--html` needs a system view, so a sync of a directory without an
/// `rticx.toml` is an error rather than a silent no-op.
fn write_html(outcome: &SyncOutcome) -> Result<PathBuf, DriverError> {
    let Some(view) = &outcome.system else {
        return Err(DriverError::NoSystemView {
            root: outcome.project_root.clone(),
        });
    };
    let path = outcome.output_dir.join(HTML_FILE);
    let html = visualize::render_html(view);
    std::fs::write(&path, html).map_err(|source| DriverError::WriteHtml {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Opens `path` in the default browser, best-effort.
///
/// `RTICX_XBIN_OPENER` overrides the platform opener (and keeps the tests
/// hermetic); otherwise `xdg-open`, `open` or `cmd /C start` is used.
fn open_in_browser(path: &Path) -> io::Result<()> {
    let status = if let Some(opener) = std::env::var_os("RTICX_XBIN_OPENER") {
        ProcessCommand::new(opener).arg(path).status()?
    } else if cfg!(target_os = "macos") {
        ProcessCommand::new("open").arg(path).status()?
    } else if cfg!(target_os = "windows") {
        ProcessCommand::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(path)
            .status()?
    } else {
        ProcessCommand::new("xdg-open").arg(path).status()?
    };

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("the opener exited with {status}")))
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

/// Reports each successful ELF verification (M6.9-T7) on standard error, like
/// [`report_ipc_types`].
///
/// Library callers inspect the `verifications` field of the outcome instead;
/// this is pure presentation. A failed verification is an error, so only
/// successes are reported here.
fn report_verifications(verifications: &[ElfVerification]) {
    for verification in verifications {
        eprintln!(
            "[cargo-xbin] verified `{}` ({} allocated sections, {} pool views)",
            verification.application, verification.sections, verification.pool_views
        );
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
