//! Compilation pass for cross-binary tasks in the RTICX multi-binary
//! extension.
//!
//! The pass has two modes (see `multibinary-multicore-plan.md` §7 and §8):
//!
//! - **metadata mode** (phase 1, `cargo xbin sync`): when
//!   `RTICX_XBIN_META_OUT` is set in the compiler environment, the pass writes
//!   `<target>.xbin.json` describing the cross-binary tasks declared by this
//!   application. The driver (`cargo xbin`) sets the variable for the
//!   `cargo check` invocation it runs per application, so the manifest is
//!   always written by the application's own macro expansion (M1-T2);
//! - **codegen mode** (phase 2): it reads the driver-generated
//!   `system.json`, filters it by this application's cores, and emits sender
//!   stubs and receiver dispatchers. That lands in M3; for now the pass only
//!   parses and strips its syntax.
//!
//! Syntax owned by this pass (always consumed, in both modes):
//!
//! - `#[app(core_ids = [..], external_cores = [..])]` — see [`crate::parse`];
//! - `#[cross_bin_task(..)]` receiver structs and `#[cross_bin_spawn(..)]`
//!   sender stubs, with their input type taken from the matching
//!   `impl CrossBinTask`/`impl CrossBinSpawn` block.
//!
//! Detection of the metadata environment happens when the pass is
//! constructed (`XbinPass::from_env`), i.e. at the macro entry point, so a
//! distribution can simply bind it unconditionally.

mod parse;

use std::path::PathBuf;

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use rticx_core::InfoBus;
use rticx_xbin_proto::{AppManifest, Hash64, MANIFEST_SCHEMA_VERSION, TargetRef};
use syn::ItemMod;

use crate::parse::{AppExtensions, ModuleDecls, parse_app_args, parse_module};

/// Re-export of the pass trait, so that distributions and fixtures binding
/// [`XbinPass`] need not depend on `rticx-core` directly.
pub use rticx_core::RticPass;

/// Environment variable that switches the pass into metadata mode.
///
/// Its value is the directory where `<target>.xbin.json` is written.
pub const META_OUT_ENV: &str = "RTICX_XBIN_META_OUT";

/// The cross-binary compilation pass.
#[derive(Debug, Clone)]
pub struct XbinPass {
    meta: Option<MetaMode>,
}

/// Metadata-mode configuration resolved at pass construction.
#[derive(Debug, Clone)]
struct MetaMode {
    /// Directory `<target>.xbin.json` is written to.
    out_dir: PathBuf,
    /// `CARGO_PKG_NAME` of the application being compiled.
    package: Option<String>,
    /// `CARGO_BIN_NAME` of the application being compiled.
    target: Option<String>,
}

impl XbinPass {
    /// Creates the pass, enabling metadata mode when [`META_OUT_ENV`] is set.
    ///
    /// This is the constructor distributions bind: cargo propagates the
    /// environment variable to the compiler, and therefore to this proc
    /// macro, only for the `cargo check` runs the driver starts.
    pub fn from_env() -> Self {
        let out_dir = std::env::var_os(META_OUT_ENV)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());
        let Some(out_dir) = out_dir else {
            return Self::disabled();
        };
        Self {
            meta: Some(MetaMode {
                out_dir,
                package: non_empty_env("CARGO_PKG_NAME"),
                target: non_empty_env("CARGO_BIN_NAME"),
            }),
        }
    }

    /// Creates a pass that never writes a manifest.
    pub fn disabled() -> Self {
        Self { meta: None }
    }

    /// Creates a metadata-mode pass with an explicit output directory,
    /// package and target (used by tests and tooling that drive the pass
    /// directly).
    pub fn with_manifest(
        out_dir: impl Into<PathBuf>,
        package: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            meta: Some(MetaMode {
                out_dir: out_dir.into(),
                package: Some(package.into()),
                target: Some(target.into()),
            }),
        }
    }

    /// Returns whether this pass was constructed in metadata mode.
    pub fn is_metadata_mode(&self) -> bool {
        self.meta.is_some()
    }
}

impl Default for XbinPass {
    fn default() -> Self {
        Self::disabled()
    }
}

impl RticPass for XbinPass {
    fn subscribe(&mut self, _info_bus: InfoBus) {}

    fn run_pass(
        &self,
        args: TokenStream,
        mut app_mod: ItemMod,
    ) -> syn::Result<(TokenStream, ItemMod)> {
        // The source hash must describe the application exactly as the user
        // wrote it, before this pass strips its own syntax, so that phase 2
        // can recompute it from the same source.
        let source_hash = source_hash(&args, &app_mod);

        let (extensions, args) = parse_app_args(args)?;
        let decls = parse_module(&mut app_mod, extensions.cores)?;

        if let Some(meta) = &self.meta {
            let manifest = build_manifest(meta, source_hash, extensions, decls)?;
            write_manifest(meta, &manifest)?;
        }

        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "rticx-xbin-pass"
    }
}

/// Builds the application manifest from the parsed syntax.
fn build_manifest(
    meta: &MetaMode,
    source_hash: Hash64,
    extensions: AppExtensions,
    decls: ModuleDecls,
) -> syn::Result<AppManifest> {
    let package = meta.package.as_deref().ok_or_else(|| {
        meta_error(
            "`RTICX_XBIN_META_OUT` is set but `CARGO_PKG_NAME` is not defined; \
             metadata collection must run through cargo (see `cargo xbin sync`)",
        )
    })?;
    let target = meta
        .target
        .as_deref()
        .filter(|target| !target.is_empty())
        .unwrap_or(package);

    let mut types: Vec<String> = decls
        .receivers
        .iter()
        .map(|receiver| receiver.input_type.clone())
        .chain(
            decls
                .senders
                .iter()
                .filter_map(|sender| sender.input_type.clone()),
        )
        .collect();
    types.sort();
    types.dedup();

    Ok(AppManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package: package.to_string(),
        target: TargetRef::bin(target),
        source_hash,
        cores: extensions.cores,
        core_ids: extensions.core_ids,
        external_cores: extensions.external_cores,
        types,
        receivers: decls.receivers,
        senders: decls.senders,
    })
}

/// Writes `<target>.xbin.json` into the configured directory.
fn write_manifest(meta: &MetaMode, manifest: &AppManifest) -> syn::Result<()> {
    let path = meta
        .out_dir
        .join(AppManifest::file_name(&manifest.target.name));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            meta_error(format!(
                "failed to create manifest directory `{}`: {source}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(&path, manifest.to_json()).map_err(|source| {
        meta_error(format!(
            "failed to write application manifest `{}`: {source}",
            path.display()
        ))
    })
}

/// Deterministic hash of the macro arguments and the annotated module.
fn source_hash(args: &TokenStream, app_mod: &ItemMod) -> Hash64 {
    let mut source = args.to_string();
    source.push('\n');
    source.push_str(&app_mod.to_token_stream().to_string());
    Hash64::of(source.as_bytes())
}

fn meta_error(message: impl Into<String>) -> syn::Error {
    syn::Error::new(Span::call_site(), message.into())
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use quote::{ToTokens, quote};
    use rticx_core::RticPass;
    use rticx_core::parse_utils::RticAttr;

    use super::XbinPass;

    #[test]
    fn disabled_pass_strips_its_syntax() {
        let pass = XbinPass::disabled();
        assert!(!pass.is_metadata_mode());

        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
                struct EncryptTask;

                impl CrossBinTask for EncryptTask {
                    type Input = ipc_types::EncryptReq;
                    fn exec(&mut self, input: Self::Input) {}
                }

                #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
                struct EncryptTask;
            }
        };
        let input_args = quote!(
            device = mypac,
            cores = 1,
            core_ids = [0],
            external_cores = [1]
        );

        let (args, out) = pass
            .run_pass(input_args, app_mod)
            .expect("parsing succeeds");
        assert_eq!(pass.pass_name(), "rticx-xbin-pass");

        let parsed = RticAttr::parse_from_tokens(args, quote::format_ident!("app"))
            .expect("stripped args stay parseable");
        assert!(parsed.get_expr("device").is_some(), "core args are kept");
        assert!(parsed.get_expr("cores").is_some(), "core args are kept");
        assert!(parsed.get_expr("core_ids").is_none(), "consumed");
        assert!(parsed.get_expr("external_cores").is_none(), "consumed");

        let tokens = out.to_token_stream().to_string();
        assert!(!tokens.contains("cross_bin_task"), "{tokens}");
        assert!(!tokens.contains("cross_bin_spawn"), "{tokens}");
        assert!(
            tokens.contains("impl CrossBinTask for EncryptTask"),
            "{tokens}"
        );
    }

    #[test]
    fn receiver_without_input_type_is_rejected() {
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                #[cross_bin_task(priority = 3)]
                struct EncryptTask;
            }
        };
        let error = XbinPass::disabled()
            .run_pass(quote!(device = mypac), app_mod)
            .expect_err("receiver needs `type Input`")
            .to_string();
        assert!(error.contains("type Input"), "{error}");
    }

    #[test]
    fn unknown_task_argument_is_rejected() {
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                #[cross_bin_spawn(core = 1, bogus = 3)]
                struct EncryptTask;
            }
        };
        let error = XbinPass::disabled()
            .run_pass(quote!(device = mypac), app_mod)
            .expect_err("unknown key")
            .to_string();
        assert!(error.contains("unknown argument `bogus`"), "{error}");
    }

    #[test]
    fn core_ids_must_cover_every_local_core() {
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {}
        };
        let error = XbinPass::disabled()
            .run_pass(quote!(device = mypac, cores = 2, core_ids = [7]), app_mod)
            .expect_err("mapping length mismatch")
            .to_string();
        assert!(error.contains("expected 2 entries, found 1"), "{error}");
    }
}
