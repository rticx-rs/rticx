//! M0-T4 acceptance test: the generated `ipc-types` crate must compile on
//! the host and on at least two embedded targets, which also evaluates every
//! layout assertion at compile time.
//!
//! The generated manifest points at `rticx-xbin-rt` by absolute path so the
//! crate can be built outside the experimental workspace.

use std::path::{Path, PathBuf};
use std::process::Command;

use rticx_xbin_proto::{CodegenOptions, RtDependency, generate_crate, parse_idl_str};

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

fn rt_crate_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rticx-xbin-rt")
}

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
        "generated crate failed to build for {target:?}"
    );
}

#[test]
fn generated_crate_compiles_on_host_and_embedded_targets() {
    let idl = parse_idl_str(IDL).expect("valid IDL");
    let options = CodegenOptions {
        rt_dependency: RtDependency::Path(
            rt_crate_path().to_str().expect("utf-8 path").to_string(),
        ),
    };
    let generated = generate_crate(&idl, &options).expect("codegen");

    let dir = tempfile::tempdir().expect("tempdir");
    generated
        .write_to(dir.path())
        .expect("write generated crate");

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
        "M0-T4 requires the generated crate to compile on at least two embedded targets, \
         found {embedded}; install them with `rustup target add thumbv7em-none-eabihf \
         thumbv6m-none-eabi`"
    );
}
