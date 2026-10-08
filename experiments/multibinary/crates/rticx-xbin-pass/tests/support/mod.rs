//! Shared helpers for the `rticx-xbin-pass` integration tests.
//!
//! Every file in `tests/` is a separate test crate, so this subdirectory is
//! shared by declaring `mod support;` in each file. Each test crate only uses
//! a subset of the helpers, hence the `dead_code` allowance.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serializes the tests that mutate the process-global environment.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Saves and restores environment variables around a test that sets them.
pub struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    /// Sets `name` to `value`, restoring the previous value on drop.
    pub fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
        Self::set_all(&[(name, value.as_ref())])
    }

    /// Sets `vars`, restoring the previous values on drop.
    pub fn set_all(vars: &[(&'static str, &OsStr)]) -> Self {
        let saved = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::set_var(name, value) };
        }
        Self { saved }
    }

    /// Removes `names` from the environment, restoring them on drop.
    ///
    /// Models compilers that leave a variable unset: rust-analyzer's
    /// proc-macro server never defines `CARGO_BIN_NAME`.
    pub fn unset_all(names: &[&'static str]) -> Self {
        let saved = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in names {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::remove_var(name) };
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// The `cargo` binary that invoked the test, or `cargo` from `PATH`.
pub fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// The canonicalized paths of the in-tree runtime and mock crates.
pub fn runtime_and_mock_paths() -> (PathBuf, PathBuf) {
    let rt = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rticx-xbin-rt")
        .canonicalize()
        .expect("the runtime crate ships with the workspace");
    let mock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/rticx-xbin-mock")
        .canonicalize()
        .expect("the mock crate ships with the workspace");
    (rt, mock)
}
