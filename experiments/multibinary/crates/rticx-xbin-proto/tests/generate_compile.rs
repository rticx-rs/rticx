//! M0-T4 acceptance test: the generated `ipc_types` module must compile on
//! the host and on at least two embedded targets, which also evaluates every
//! layout assertion at compile time.
//!
//! The module is wrapped exactly the way the cross-binary pass wraps it: a
//! `CrossCoreMessage` trait plus `pub mod ipc_types { use super::…; … }` in a
//! `#![no_std]` crate with **no** `rticx-xbin-rt` dependency.

use std::path::{Path, PathBuf};
use std::process::Command;

use rticx_xbin_proto::{generate_module, parse_idl_str};

const IDL: &str = r#"
schema = 1

[message.Point]
fields = { x = "i32", y = "i32" }

[message.EncryptReq]
fields = { addr = "u32", len = "u32", key = "u32" }

[message.SensorBatch]
fields = { tag = "u8", kind = "Kind", xs = "[i16; 8]", origin = "Point", points = "[Point; 2]" }

[enum.Kind]
variants = { Fast = 0, Slow = 1 }
"#;

/// Embedded targets tried in order; at least two must be installed.
const EMBEDDED_TARGETS: &[&str] = &[
    "thumbv7em-none-eabihf",
    "thumbv6m-none-eabi",
    "riscv32imc-unknown-none-elf",
];

const WRAPPER_CARGO_TOML: &str = "\
[package]
name = \"generated-ipc-types-wrapper\"
version = \"0.0.0\"
edition = \"2021\"
publish = false

[lib]
path = \"src/lib.rs\"
";

const WRAPPER_LIB_RS: &str = "\
#![no_std]

pub unsafe trait CrossCoreMessage: Copy + 'static {}

#[allow(non_camel_case_types, non_snake_case)]
pub mod ipc_types {
    use super::CrossCoreMessage;
    include!(\"ipc_types.rs\");
}
";

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

fn rustc() -> PathBuf {
    std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"))
}

fn target_installed(target: &str) -> bool {
    let output = Command::new(rustc())
        .args(["--print", "sysroot"])
        .output()
        .expect("failed to run rustc --print sysroot");
    let sysroot = String::from_utf8(output.stdout).expect("rustc output is utf-8");
    Path::new(sysroot.trim())
        .join("lib/rustlib")
        .join(target)
        .join("lib")
        .is_dir()
}

fn write_wrapper(dir: &Path, generated: &str) {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("create src");
    std::fs::write(dir.join("Cargo.toml"), WRAPPER_CARGO_TOML).expect("write Cargo.toml");
    std::fs::write(src.join("lib.rs"), WRAPPER_LIB_RS).expect("write lib.rs");
    std::fs::write(src.join("ipc_types.rs"), generated).expect("write ipc_types.rs");
}

fn build(dir: &Path, target: Option<&str>) {
    let mut command = Command::new(cargo());
    command
        .arg("build")
        .arg("--quiet")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", dir.join("target"));
    if let Some(target) = target {
        command.arg("--target").arg(target);
    }
    let status = command.status().expect("failed to run cargo");
    assert!(
        status.success(),
        "generated module failed to build for {target:?}"
    );
}

#[test]
fn generated_module_compiles_on_host_and_embedded_targets() {
    let idl = parse_idl_str(IDL).expect("valid IDL");
    let generated = generate_module(&idl).expect("codegen");

    let dir = tempfile::tempdir().expect("tempdir");
    write_wrapper(dir.path(), &generated);

    build(dir.path(), None);

    let mut embedded = 0;
    for target in EMBEDDED_TARGETS {
        if target_installed(target) {
            build(dir.path(), Some(target));
            embedded += 1;
        }
    }

    assert!(
        embedded >= 2,
        "M0-T4 requires the generated module to compile on at least two embedded targets, \
         found {embedded}; install them with `rustup target add thumbv7em-none-eabihf \
         thumbv6m-none-eabi`"
    );
}
