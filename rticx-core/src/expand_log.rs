//! Best-effort expansion logging, driven by environment variables.
//!
//! When `RTICX_EXPAND` is set (any value), the macro pipeline writes its
//! expansions to disk so that application developers can inspect them (GDB
//! stepping, security vetting) and pass/distribution developers can debug
//! their syntax transformations.
//!
//! Everything here is intentionally best-effort: writes never fail the macro
//! expansion. If no output directory can be determined, logging is disabled.
//!
//! # Environment variables
//!
//! - `RTICX_EXPAND` — trigger. When unset, no expansion logging happens.
//! - `RTICX_EXPAND_PATH` — optional directory override. Defaults to
//!   `$CARGO_TARGET_DIR/rticx-expand`, falling back to
//!   `$CARGO_MANIFEST_DIR/target/rticx-expand`.
//! - `RTICX_EXPAND_PASS_DIR` — optional directory for ordered per-stage
//!   snapshots of the module. One snapshot is written after every pipeline
//!   stage (`00_original.rs`, `01_<PassName>.rs`, …, `NN_core.rs`) so that
//!   consecutive files can be diffed to see exactly what each pass (and the
//!   core pass) changed.

use std::path::PathBuf;

use proc_macro2::TokenStream as TokenStream2;
use quote::ToTokens;

const TRIGGER_VAR: &str = "RTICX_EXPAND";
const DIR_OVERRIDE_VAR: &str = "RTICX_EXPAND_PATH";
const PASS_DIR_VAR: &str = "RTICX_EXPAND_PASS_DIR";

/// Writes expansions for a single macro invocation to a dedicated directory.
///
/// Files are named `<stem>_<label>.rs` where `<stem>` is derived from the
/// file that invoked the macro (or `CARGO_BIN_NAME` / `CARGO_PKG_NAME` as
/// fallback), e.g. `main_expanded.rs`.
pub struct ExpandLog {
    dir: PathBuf,
    stem: String,
    /// When set, ordered per-stage snapshots are written here.
    pass_dir: Option<PathBuf>,
}

impl ExpandLog {
    /// Returns `None` when expansion logging is not requested (`RTICX_EXPAND`
    /// unset) or when no output directory can be determined.
    pub fn from_env(source_file: Option<PathBuf>) -> Option<Self> {
        std::env::var_os(TRIGGER_VAR)?;

        let dir = std::env::var_os(DIR_OVERRIDE_VAR)
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("CARGO_TARGET_DIR").map(|d| PathBuf::from(d).join("rticx-expand"))
            })
            .or_else(|| {
                std::env::var_os("CARGO_MANIFEST_DIR")
                    .map(|d| PathBuf::from(d).join("target").join("rticx-expand"))
            })?;

        let stem = source_file
            .and_then(|path| {
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                // Ignore virtual span files (e.g. `<anon>`) produced when the
                // macro is invoked from another macro expansion.
                (!stem.is_empty() && !stem.starts_with('<')).then_some(stem)
            })
            .or_else(|| non_empty_env("CARGO_BIN_NAME"))
            .or_else(|| non_empty_env("CARGO_PKG_NAME"))
            .unwrap_or_else(|| "app".to_string());

        let pass_dir = std::env::var_os(PASS_DIR_VAR)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty());

        Some(Self {
            dir,
            stem,
            pass_dir,
        })
    }

    /// Whether per-stage snapshot logging is enabled.
    pub fn pass_dir(&self) -> Option<&std::path::Path> {
        self.pass_dir.as_deref()
    }

    /// Writes a per-stage snapshot to the pass directory as `NN_<label>.rs`,
    /// zero-padded so that lexical order equals pipeline order.
    pub(crate) fn write_pass_state(&self, index: usize, label: &str, contents: &str) {
        let Some(pass_dir) = &self.pass_dir else {
            return;
        };
        let Ok(()) = std::fs::create_dir_all(pass_dir) else {
            return;
        };
        let file_name = format!("{index:02}_{}.rs", sanitize(label));
        let _ = std::fs::write(pass_dir.join(file_name), contents.as_bytes());
    }

    /// Writes a rendered expansion to `<stem>_<label>.rs`.
    pub fn write(&self, label: &str, code: &TokenStream2) {
        self.write_raw(label, "rs", &code.to_string());
    }

    fn write_raw(&self, label: &str, extension: &str, contents: &str) {
        if self.dir.as_os_str().is_empty() {
            return;
        }
        let Ok(()) = std::fs::create_dir_all(&self.dir) else {
            return;
        };
        let file_name = format!("{}_{}.{}", self.stem, sanitize(label), extension);
        let _ = std::fs::write(self.dir.join(file_name), contents.as_bytes());
    }
}

fn non_empty_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|value| !value.is_empty())
}

/// Makes `label` safe to embed in a file name.
fn sanitize(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Renders a mid-pipeline state (macro args + annotated module) for
/// inspection, e.g. the output of a pass or the input of a failing pass.
pub fn render_pass_state(context: &str, args: &TokenStream2, app_mod: &syn::ItemMod) -> String {
    let app_mod = app_mod.to_token_stream();
    format!(
        "// ==================== {context} ====================\n\
         // #[app({})]\n\
         {}\n",
        args, app_mod,
    )
}
