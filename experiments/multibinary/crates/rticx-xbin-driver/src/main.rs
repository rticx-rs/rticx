//! `cargo-xbin` binary entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    ExitCode::from(rticx_xbin_driver::main_entry(argv) as u8)
}
