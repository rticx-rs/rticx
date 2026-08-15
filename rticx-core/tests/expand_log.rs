//! Tests for the env-driven expansion logging (`RTICX_EXPAND` and friends).
//!
//! Environment variables are process-global, so all tests in this file share
//! a mutex and restore the environment afterwards.

mod common;

use std::sync::Mutex;

use proc_macro2::TokenStream;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{InfoBus, RticMacroBuilder, RticPass};
use syn::ItemMod;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Saves the expansion-related environment variables so tests can restore them.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: single-threaded test process (guarded by ENV_LOCK).
        unsafe {
            std::env::remove_var("RTICX_EXPAND");
            std::env::remove_var("RTICX_EXPAND_PATH");
            std::env::remove_var("RTICX_EXPAND_PASS_DIR");
            std::env::remove_var("CARGO_BIN_NAME");
        }
    }
}

fn set_expand_env(dir: &std::path::Path) -> EnvGuard {
    // SAFETY: single-threaded test process (guarded by ENV_LOCK).
    unsafe {
        std::env::set_var("RTICX_EXPAND", "1");
        std::env::set_var("RTICX_EXPAND_PATH", dir);
        std::env::set_var("CARGO_BIN_NAME", "expand_test");
    }
    EnvGuard
}

fn set_pass_dir(dir: &std::path::Path) {
    // SAFETY: single-threaded test process (guarded by ENV_LOCK).
    unsafe { std::env::set_var("RTICX_EXPAND_PASS_DIR", dir) };
}

/// A pass that just hands the tokens through (optionally failing).
struct IdentityPass {
    name: &'static str,
    fail: bool,
}

impl RticPass for IdentityPass {
    fn subscribe(&mut self, _info_bus: InfoBus) {}

    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        if self.fail {
            return Err(syn::Error::new_spanned(&app_mod, "test failure"));
        }
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        self.name
    }
}

#[test]
fn expansion_disabled_without_trigger_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let _env = set_expand_env(dir.path());
    // SAFETY: single-threaded test process (guarded by ENV_LOCK).
    unsafe { std::env::remove_var("RTICX_EXPAND") };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let _ = builder.build_rtic_macro2(
        common::single_core_app_args(),
        common::single_core_app_module(),
        None,
    );

    assert!(
        dir.path().read_dir().is_ok_and(|e| e.count() == 0),
        "no expansion files should be written without RTICX_EXPAND"
    );
}

#[test]
fn writes_full_expansion_without_pass_dir() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let _env = set_expand_env(dir.path());

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(IdentityPass {
        name: "MockPass",
        fail: false,
    });
    let code = builder.build_rtic_macro2(
        common::single_core_app_args(),
        common::single_core_app_module(),
        None,
    );

    // The pipeline must have run to completion (no compile_error in the output).
    assert!(
        !code.to_string().contains("compile_error"),
        "pipeline failed: {code}"
    );

    let full = std::fs::read_to_string(dir.path().join("expand_test_expanded.rs")).unwrap();
    assert!(full.contains("pub mod app"), "full expansion written");

    // No per-stage snapshots without RTICX_EXPAND_PASS_DIR.
    let pass_dir = dir.path().join("passes");
    assert!(!pass_dir.exists());
}

#[test]
fn writes_ordered_snapshots_after_every_stage() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let _env = set_expand_env(dir.path());
    let pass_dir = dir.path().join("passes");
    set_pass_dir(&pass_dir);

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(IdentityPass {
        name: "FirstPass",
        fail: false,
    });
    builder.bind_pre_core_pass(IdentityPass {
        name: "SecondPass",
        fail: false,
    });
    let code = builder.build_rtic_macro2(
        common::single_core_app_args(),
        common::single_core_app_module(),
        None,
    );
    assert!(
        !code.to_string().contains("compile_error"),
        "pipeline failed"
    );

    let snapshot = |name: &str| {
        std::fs::read_to_string(pass_dir.join(name)).unwrap_or_else(|e| {
            panic!(
                "missing snapshot {name}: {e}; dir = {:?}",
                std::fs::read_dir(&pass_dir)
            )
        })
    };

    let original = snapshot("00_original.rs");
    assert!(
        original.contains("original app module (before all passes)"),
        "baseline snapshot written"
    );
    assert!(snapshot("01_FirstPass.rs").contains("output of `FirstPass`"));
    assert!(snapshot("02_SecondPass.rs").contains("output of `SecondPass`"));
    let core = snapshot("03_core.rs");
    assert!(
        core.contains("pub mod app"),
        "core snapshot is the final expansion"
    );

    // The core snapshot must match the full expansion file.
    let full = std::fs::read_to_string(dir.path().join("expand_test_expanded.rs")).unwrap();
    assert_eq!(core, full);
}

#[test]
fn writes_input_of_failing_pass() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let _env = set_expand_env(dir.path());
    let pass_dir = dir.path().join("passes");
    set_pass_dir(&pass_dir);

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(IdentityPass {
        name: "FailingPass",
        fail: true,
    });
    let code = builder.build_rtic_macro2(
        common::single_core_app_args(),
        common::single_core_app_module(),
        None,
    );

    assert!(code.to_string().contains("compile_error"), "pass must fail");

    let input = std::fs::read_to_string(pass_dir.join("01_FailingPass_input.rs")).unwrap();
    assert!(
        input.contains("input to `FailingPass` (pass failed)"),
        "input of failing pass written"
    );
    // The pass never ran, so no output snapshot and no full expansion exist.
    assert!(!pass_dir.join("01_FailingPass.rs").exists());
    assert!(!dir.path().join("expand_test_expanded.rs").exists());
}
