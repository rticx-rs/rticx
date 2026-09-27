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
//! - **codegen mode** (phase 2): it reads the driver-generated `system.json`
//!   (from `RTICX_XBIN_SYSTEM`, or the default
//!   `<project root>/target/rticx-xbin/system.json`), filters it by this
//!   application's cores, and emits the cross-binary code:
//!
//!   - sender side (M3-T1, M5.5): a generated sender stub
//!     (`pub struct <Task>;`) plus FIFO views and `Task::cross_spawn` for
//!     every view task whose `spawner_core` belongs to this application —
//!     producer sources declare nothing;
//!   - receiver side (M3-T2, M5.5): FIFO views, the
//!     `SpawnInput: CrossCoreMessage` const assertion, one generated doorbell
//!     dispatcher per `(source -> target, priority)` line, and the core
//!     `#[task(..)]` shape on the native receiver structs themselves (see
//!     `crate::parse::inject_receiver_tasks`);
//!   - init hooks (M3-T3): `__rticx_xbin_configure_shared_memory` on every
//!     core, `__rticx_xbin_init_shared` on the application owning the
//!     project's owner core, and `__rticx_xbin_mark_ready_core<N>` on every
//!     local core, wired into the generated entry functions through
//!     [`RticPass::main_injection`] (see `crate::codegen`);
//!   - freshness (M3-T4): the generated code embeds the synced
//!     `__RTICX_XBIN_TOPOLOGY_HASH` and `include_str!`s the system view, so
//!     rustc records it in dep-info and rebuilds the application when it
//!     changes. The pass hard-errors on a `topology_hash` that does not match
//!     the view's contents (it was edited after `cargo xbin sync`) and on an
//!     application whose source changed since the last `sync` (see
//!     `crate::codegen::check_source_hash`).
//!
//! Codegen mode loads the view for every application listed in it — not only
//! for applications declaring cross tasks — because producer endpoints need
//! their generated stubs and every application needs its init hooks (M5.5).
//!
//! Metadata mode wins when both environments are configured, so the
//! `cargo check` runs of `cargo xbin sync` never generate code. Code
//! generation needs a distribution backend ([`XbinPassBackend`]): a
//! distribution binds it with [`XbinPass::with_backend`].
//!
//! The pass is bound **before** `rticx-sw-pass` and requires the
//! distribution's `swtasks` feature: the software pass emits the generated
//! `RticSwTask` trait and the core pass's external `task_trait` mechanism
//! compiles the receiver through it. Without the feature `RticSwTask` is
//! undefined and the receiver fails to compile. Async cross-binary tasks are
//! out of scope until a later milestone.
//!
//! Syntax owned by this pass:
//!
//! - `#[app(external_cores = [..], ipc_dispatchers = [..])]` (always consumed,
//!   in both modes); `#[app(core_ids = [..])]` is *read* through the core
//!   parser and left for the core pass, which owns the key since M5 — see
//!   `crate::parse`. `ipc_dispatchers` is the per-core interrupt pool of the
//!   generated cross-binary line dispatchers (M6.5-T1);
//! - native `#[sw_task(..)]` cross-binary receivers (always consumed, in both
//!   modes), with their input type taken from the matching `impl RticSwTask {
//!   type SpawnInput = …; }` block. In an application declaring
//!   `external_cores`, `spawn_by` is resolved in the global namespace (see
//!   `crate::parse`); there are no sender declarations since M5.5.
//!
//! Detection of the metadata environment happens when the pass is
//! constructed (`XbinPass::from_env`), i.e. at the macro entry point, so a
//! distribution can simply bind it unconditionally.

mod codegen;
mod parse;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use rticx_core::{InfoBus, MainInjectionPoint};
use rticx_xbin_proto::{AppManifest, Hash64, MANIFEST_SCHEMA_VERSION, SystemView, TargetRef};
use syn::ItemMod;

use crate::codegen::{
    HookPlan, check_application, check_source_hash, generate_freshness_items, generate_init_hooks,
    generate_receiver_items, generate_sender_items,
};
use crate::parse::{
    AppExtensions, ModuleDecls, inject_receiver_tasks, parse_app_args, parse_module,
    strip_receiver_tasks, validate_ipc_dispatchers,
};

/// Re-export of the pass trait, so that distributions and fixtures binding
/// [`XbinPass`] need not depend on `rticx-core` directly.
pub use rticx_core::RticPass;

pub use crate::codegen::XbinPassBackend;

/// Environment variable that switches the pass into metadata mode.
///
/// Its value is the directory where `<target>.xbin.json` is written.
pub const META_OUT_ENV: &str = "RTICX_XBIN_META_OUT";

/// Environment variable naming the `system.json` to generate against.
///
/// When unset, the pass looks for `rticx.toml` in `CARGO_MANIFEST_DIR` and its
/// parents and uses `<project root>/target/rticx-xbin/system.json`.
pub const SYSTEM_ENV: &str = "RTICX_XBIN_SYSTEM";

/// `rticx.toml` file name, used to discover the project root.
///
/// TODO(extract): mirrors `rticx_xbin_driver::PROJECT_MANIFEST`; the driver
/// depends on this crate, so the constant cannot be shared yet.
const PROJECT_MANIFEST: &str = "rticx.toml";

/// Default system view path below the project root.
///
/// TODO(extract): mirrors `rticx_xbin_driver`'s `SYSTEM_FILE` and output
/// directory.
const SYSTEM_FILE: &str = "system.json";

/// The cross-binary compilation pass.
pub struct XbinPass {
    meta: Option<MetaMode>,
    system: Option<SystemMode>,
    backend: Option<Box<dyn XbinPassBackend>>,
    /// Init-hook wiring stashed by `run_pass` and read back by
    /// `main_injection` (both take `&self`).
    hooks: RefCell<Option<HookPlan>>,
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

/// Codegen-mode configuration resolved at pass construction.
#[derive(Debug, Clone)]
struct SystemMode {
    /// Path of the driver-generated `system.json`.
    path: PathBuf,
    /// `CARGO_PKG_NAME` of the application being compiled.
    package: Option<String>,
    /// `CARGO_BIN_NAME` of the application being compiled.
    target: Option<String>,
}

impl std::fmt::Debug for XbinPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XbinPass")
            .field("metadata_mode", &self.meta.is_some())
            .field("system", &self.system)
            .field("backend", &self.backend.is_some())
            .field("init_hooks", &self.hooks.borrow().is_some())
            .finish()
    }
}

impl XbinPass {
    /// Creates the pass, detecting its mode from the environment:
    ///
    /// - [`META_OUT_ENV`] set: **metadata mode** (phase 1);
    /// - otherwise [`SYSTEM_ENV`] set, or a `rticx.toml` discovered above
    ///   `CARGO_MANIFEST_DIR`: **codegen mode** (phase 2). The view is loaded
    ///   for every application listed in it; a missing file is the documented
    ///   "run `cargo xbin sync`" hard error, never a silent skip of code
    ///   generation;
    /// - neither: the pass only parses and strips its syntax.
    ///
    /// This is the constructor distributions bind: cargo propagates the
    /// environment variables to the compiler, and therefore to this proc
    /// macro, only for the invocations the driver starts. Metadata mode wins
    /// when both are configured, so the metadata `cargo check` builds never
    /// generate code; add the distribution backend with
    /// [`XbinPass::with_backend`].
    pub fn from_env() -> Self {
        if let Some(out_dir) = non_empty_env_os(META_OUT_ENV).map(PathBuf::from) {
            return Self {
                meta: Some(MetaMode {
                    out_dir,
                    package: non_empty_env("CARGO_PKG_NAME"),
                    target: non_empty_env("CARGO_BIN_NAME"),
                }),
                system: None,
                backend: None,
                hooks: RefCell::new(None),
            };
        }
        Self {
            meta: None,
            system: system_from_env(),
            backend: None,
            hooks: RefCell::new(None),
        }
    }

    /// Creates a pass that never writes a manifest and never generates code.
    pub fn disabled() -> Self {
        Self {
            meta: None,
            system: None,
            backend: None,
            hooks: RefCell::new(None),
        }
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
            system: None,
            backend: None,
            hooks: RefCell::new(None),
        }
    }

    /// Creates a codegen-mode pass reading `system.json` from `path`.
    ///
    /// Used by tests and tooling that drive the pass directly; distributions
    /// normally use [`XbinPass::from_env`] and only add
    /// [`XbinPass::with_backend`].
    pub fn with_system(
        path: impl Into<PathBuf>,
        package: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            meta: None,
            system: Some(SystemMode {
                path: path.into(),
                package: Some(package.into()),
                target: Some(target.into()),
            }),
            backend: None,
            hooks: RefCell::new(None),
        }
    }

    /// Configures the distribution backend used to generate runtime code.
    ///
    /// Code generation for an application with cross-binary endpoints (a
    /// receiver or a view task it produces) requires it; a distribution binds
    /// it unconditionally while its macro runs.
    pub fn with_backend<T: XbinPassBackend + 'static>(mut self, backend: T) -> Self {
        self.backend = Some(Box::new(backend));
        self
    }

    /// Returns whether this pass was constructed in metadata mode.
    pub fn is_metadata_mode(&self) -> bool {
        self.meta.is_some()
    }

    /// Returns whether this pass will generate code from a `system.json`.
    pub fn is_codegen_mode(&self) -> bool {
        self.system.is_some()
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
        let source_hash = app_source_hash(&args, &app_mod);

        let (extensions, args) = parse_app_args(args)?;
        let decls = parse_module(&mut app_mod, &extensions)?;

        // The pass-owned `ipc_dispatchers` pool is consumed in both modes: the
        // core/software/async passes must never see it. Its entries are
        // matched against the cross lines of the parsed declarations before
        // either mode runs, so a missing, extra, duplicate or overlapping
        // entry is a hard error (M6.5-T1).
        validate_ipc_dispatchers(&extensions, &decls)?;

        if let Some(meta) = &self.meta {
            let manifest = build_manifest(meta, source_hash, extensions.clone(), decls.clone())?;
            write_manifest(meta, &manifest)?;

            // Metadata mode does not run the software pass: strip the parsed
            // cross-receiver attributes so the emitted module only keeps the
            // struct and its `impl RticSwTask` block; the manifest already
            // recorded the task.
            strip_receiver_tasks(&mut app_mod, &decls.receivers);
        }

        if let Some(system) = &self.system {
            let package = system.package.as_deref().ok_or_else(|| {
                meta_error(
                    "the system view is configured but `CARGO_PKG_NAME` is not defined; \
                     code generation must run through cargo",
                )
            })?;
            let target = system
                .target
                .as_deref()
                .filter(|target| !target.is_empty())
                .unwrap_or(package);

            // Every application listed in the view loads it: producer
            // endpoints need their generated stubs and every application
            // needs its init hooks, even when it declares no cross task
            // (M5.5).
            let view = load_system(&system.path)?;
            let application = check_application(&view, package, target, &extensions)?;
            let mut items =
                generate_sender_items(&app_mod, &view, application, self.backend.as_deref())?;
            items.extend(generate_receiver_items(
                &view,
                application,
                &decls.receivers,
                self.backend.as_deref(),
            )?);

            // Freshness runs after the declaration checks so a source change
            // that also breaks a task declaration reports the specific
            // mismatch first; both errors point at `cargo xbin sync`.
            check_source_hash(application, source_hash)?;
            items.extend(generate_freshness_items(&view, &system.path)?);

            // The init hooks need the backend for the same reason the sender
            // and receiver items do; without one the freshness anchors still
            // compile, but no runtime hooks are wired.
            if let Some(backend) = self.backend.as_deref() {
                let hooks = generate_init_hooks(&view, application, backend)?;
                items.extend(hooks.items);
                *self.hooks.borrow_mut() = Some(hooks.plan);
            }

            inject_receiver_tasks(&mut app_mod, &decls.receivers);
            append_items(&mut app_mod, items);
        }

        Ok((args, app_mod))
    }

    fn main_injection(&self, point: &MainInjectionPoint, core: u32) -> Option<TokenStream> {
        let hooks = self.hooks.borrow();
        let plan = hooks.as_ref()?;
        let backend = self.backend.as_deref()?;
        plan.injection(point, core, backend)
    }

    fn pass_name(&self) -> &str {
        "rticx-xbin-pass"
    }
}

/// Appends generated items to the `#[app]` module.
fn append_items(app_mod: &mut ItemMod, items: Vec<syn::Item>) {
    if let Some((_, module_items)) = app_mod.content.as_mut() {
        module_items.extend(items);
    }
}

/// Resolves the codegen-mode configuration from the environment.
///
/// Both an explicit [`SYSTEM_ENV`] path and a `rticx.toml` discovered above
/// `CARGO_MANIFEST_DIR` select codegen mode, whether or not the view exists:
/// an application listed in the project but without a synced view must fail
/// with the "run `cargo xbin sync`" error instead of silently generating
/// nothing (M5.5 removed the cross-declaration gate).
fn system_from_env() -> Option<SystemMode> {
    let package = non_empty_env("CARGO_PKG_NAME");
    let target = non_empty_env("CARGO_BIN_NAME");

    if let Some(path) = non_empty_env_os(SYSTEM_ENV) {
        return Some(SystemMode {
            path: PathBuf::from(path),
            package,
            target,
        });
    }

    let root = project_root_from_env()?;
    Some(SystemMode {
        path: root.join("target").join("rticx-xbin").join(SYSTEM_FILE),
        package,
        target,
    })
}

/// Walks up from `CARGO_MANIFEST_DIR` to the nearest directory holding a
/// `rticx.toml`.
fn project_root_from_env() -> Option<PathBuf> {
    let mut dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    loop {
        if dir.join(PROJECT_MANIFEST).is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Reads and parses the driver-generated system view, rejecting a stale
/// `topology_hash` (M3-T4).
///
/// The pass recomputes the topology hash from the view it just loaded and
/// compares it with the `topology_hash` field stored in the JSON: a mismatch
/// means the view was edited without a full `cargo xbin sync` (the driver is
/// the only merger, so a hand-edited view is out of contract). `cargo xbin
/// build` re-syncs first and therefore never trips this.
fn load_system(path: &Path) -> syn::Result<SystemView> {
    let source = std::fs::read_to_string(path).map_err(|source| {
        meta_error(format!(
            "failed to read the system view `{}`: {source}; run `cargo xbin sync`",
            path.display()
        ))
    })?;
    let view = SystemView::from_json(&source).map_err(|source| {
        meta_error(format!(
            "failed to parse the system view `{}`: {source}; run `cargo xbin sync`",
            path.display()
        ))
    })?;
    if !view.verify_topology_hash() {
        return Err(meta_error(format!(
            "the system view `{}` is stale: its `topology_hash` ({}) does not match its contents \
             ({}); run `cargo xbin sync`",
            path.display(),
            view.topology_hash,
            view.compute_topology_hash()
        )));
    }
    Ok(view)
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
        .collect();
    types.sort();
    types.dedup();

    Ok(AppManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package: package.to_string(),
        target: TargetRef::bin(target),
        source_hash,
        cores: extensions.cores,
        // The pass records the mapping the core pass resolves (the identity
        // `0..cores` when the application does not declare `core_ids`), so
        // the driver always validates it against `rticx.toml` (M5-T3).
        core_ids: Some(extensions.core_ids),
        external_cores: extensions.external_cores,
        types,
        receivers: decls.receivers,
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
///
/// This is the `source_hash` recorded in the application manifest and in
/// `system.json` (M1-T2). Phase 2 recomputes it with the same function and
/// rejects an application that changed after the last `cargo xbin sync`
/// (M3-T4). It is public so tests and tooling can build views that match a
/// given source.
pub fn app_source_hash(args: &TokenStream, app_mod: &ItemMod) -> Hash64 {
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

/// Returns `name`, or `None` when unset or empty.
fn non_empty_env_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use quote::{ToTokens, quote};
    use rticx_core::RticPass;
    use rticx_core::parse_utils::RticAttr;
    use rticx_core::parser::ast::AppArgs;

    use super::XbinPass;

    #[test]
    fn disabled_pass_strips_its_syntax() {
        let pass = XbinPass::disabled();
        assert!(!pass.is_metadata_mode());

        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                struct EncryptReq;

                #[sw_task(priority = 3, capacity = 2, spawn_by = 1)]
                struct EncryptTask;

                impl RticSwTask for EncryptTask {
                    type SpawnInput = EncryptReq;
                    fn exec(&mut self, input: Self::SpawnInput) {}
                }
            }
        };
        let input_args = quote!(
            device = mypac,
            cores = 1,
            core_ids = [0],
            external_cores = [1],
            ipc_dispatchers = [IRQ0]
        );

        let (args, out) = pass
            .run_pass(input_args, app_mod)
            .expect("parsing succeeds");
        assert_eq!(pass.pass_name(), "rticx-xbin-pass");

        let parsed = RticAttr::parse_from_tokens(args.clone(), quote::format_ident!("app"))
            .expect("stripped args stay parseable");
        assert!(parsed.get_expr("device").is_some(), "core args are kept");
        assert!(parsed.get_expr("cores").is_some(), "core args are kept");
        assert!(
            parsed.get_expr("core_ids").is_some(),
            "the core pass owns `core_ids` (M5-T3)"
        );
        assert!(parsed.get_expr("external_cores").is_none(), "consumed");
        assert!(
            parsed.get_expr("ipc_dispatchers").is_none(),
            "the pass-owned dispatcher pool is consumed in both modes (M6.5-T1)"
        );

        let app_args = AppArgs::parse(args).expect("the core pass parses the kept `core_ids`");
        assert_eq!(app_args.core_ids, [0]);

        // The native cross receiver keeps its `#[sw_task]` attribute (minus
        // the external `spawn_by`) and its `impl RticSwTask` block.
        let tokens = out.to_token_stream().to_string();
        assert!(!tokens.contains("spawn_by"), "{tokens}");
        assert!(
            tokens.contains("impl RticSwTask for EncryptTask"),
            "{tokens}"
        );
    }

    #[test]
    fn receiver_without_spawn_input_is_rejected() {
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                #[sw_task(priority = 3, spawn_by = 1)]
                struct EncryptTask;
            }
        };
        let error = XbinPass::disabled()
            .run_pass(
                quote!(
                    device = mypac,
                    cores = 1,
                    core_ids = [0],
                    external_cores = [1]
                ),
                app_mod,
            )
            .expect_err("receiver needs `type SpawnInput`")
            .to_string();
        assert!(error.contains("type SpawnInput"), "{error}");
    }

    #[test]
    fn unknown_task_argument_is_rejected() {
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                struct EncryptReq;

                #[sw_task(priority = 3, spawn_by = 1, bogus = 3)]
                struct EncryptTask;

                impl RticSwTask for EncryptTask {
                    type SpawnInput = EncryptReq;
                }
            }
        };
        let error = XbinPass::disabled()
            .run_pass(
                quote!(
                    device = mypac,
                    cores = 1,
                    core_ids = [0],
                    external_cores = [1]
                ),
                app_mod,
            )
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
