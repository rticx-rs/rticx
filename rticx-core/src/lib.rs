extern crate proc_macro;

use proc_macro::TokenStream;

use proc_macro2::{Ident, TokenStream as TokenStream2};
use syn::{ItemMod, parse_macro_input};

pub use common_internal::rticx_functions;
pub use common_internal::rticx_traits;

pub use analysis::{Analysis, SubAnalysis};
pub use backend::CorePassBackend;
use codegen::CodeGen;
use expand_log::{ExpandLog, render_pass_state};
pub use parser::{
    App, SubApp,
    ast::{AppArgs, RticTask},
};

pub use crate::info_bus::InfoBus;

pub mod analysis;
mod backend;
pub mod codegen;
mod common_internal;
pub mod errors;
pub mod expand_log;
pub mod mock_backend;
pub mod parse_utils;

pub mod info_bus;
pub mod parser;

/// Points in the generated `main()` where passes can inject code.
pub enum MainInjectionPoint {
    /// Inside the `interrupt_free` block, before system_init
    BeforeInit,
    /// Inside the `interrupt_free` block, after system_init + task_init, before post_init
    BeforePostInit,
    /// After the `interrupt_free` block, before the idle loop
    BeforeIdle,
}

/// Collected token streams from passes, keyed by injection point.
#[derive(Default)]
pub struct MainInjections {
    pub before_init: Vec<TokenStream2>,
    pub before_post_init: Vec<TokenStream2>,
    pub before_idle: Vec<TokenStream2>,
}

/// A trait that allows defining a **Compilation Pass**.
///
/// A **Compilation Pass** can be thought of as a (partial) proc-macro that expands parts of the user application
/// once all the compilation passes provided using [RticMacroBuilder::bind_pre_core_pass] and
/// [RticMacroBuilder::bind_post_core_pass] are run. The resulting code should be comprised only of *init*, *idle* ,
/// *shared resources* and *tasks* (that may be bound to interrupts) that share those resources. The **Core Pass**
/// then will take over from there to generate all the necessary logic and expand the user application further to an
/// application understandable by the Rust compiler.
pub trait RticPass {
    /// Subscribe to information bus where this and other passes can publish/get information to/from
    /// This function is guaranteed to be called before any other functions in this trait
    fn subscribe(&mut self, info_bus: InfoBus);

    /// Runs the (partial) proc-macro logic that allows extending the basic RTIC syntax
    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    /// Returns a human readable name/alias used to identify the pass. This identifier will show up in errors for example
    /// to help knowing exactly which compilation pass has failed in that case.
    fn pass_name(&self) -> &str;

    /// Return tokens to inject into `main()` at the given injection point.
    /// Called after all passes have run and before the core pass generates `main()`.
    fn main_injection(&self, _point: &MainInjectionPoint) -> Option<TokenStream2> {
        None
    }
}

/// This should be used to compose an **RTIC distribution**. In other words, it allows building the RTIC **app** macro
/// By providing the necessary low-level hardware bindings and binding additional **Compilation Passes**
/// in the case syntax extensions are desired.
pub struct RticMacroBuilder {
    core: Box<dyn CorePassBackend>,
    pre_std_passes: Vec<Box<dyn RticPass>>,
    info_bus: InfoBus,
}

impl RticMacroBuilder {
    pub fn new<T: CorePassBackend + 'static>(core_impl: T) -> Self {
        Self {
            core: Box::new(core_impl),
            pre_std_passes: Vec::new(),
            info_bus: InfoBus::new(),
        }
    }

    /// Binds a **Compilation Pass** that will run before the **Core Pass**
    pub fn bind_pre_core_pass<P: RticPass + 'static>(&mut self, pass: P) -> &mut Self {
        self.pre_std_passes.push(Box::new(pass));
        self
    }

    /// Once the **CorePass** low level hardware bindings are provided, and a selection of
    /// **Compilation Passes** are bound too, use this method to run the **app** proc macro logic.
    ///
    /// Returns a `proc_macro::TokenStream` of the expanded user application. This is the entry
    /// point used by distribution proc-macros.
    pub fn build_rtic_macro(self, args: TokenStream, input: TokenStream) -> TokenStream {
        // The first token of the annotated item carries a span from the file
        // the user invoked the macro from. Use it to derive where expansion
        // files should be written when `RTICX_EXPAND` is set.
        let source_file = input
            .clone()
            .into_iter()
            .next()
            .and_then(|token| token.span().local_file());
        let args = TokenStream2::from(args);
        let app_mod = parse_macro_input!(input as ItemMod);
        self.build_rtic_macro2(args, app_mod, source_file).into()
    }

    /// Same as [build_rtic_macro] but operates on `proc_macro2` types.
    ///
    /// This method is exposed so that tests and downstream tooling can drive the core pipeline
    /// without needing a proc-macro context. `source_file` is the file the macro was invoked
    /// from (if known) and is used to name expansion files when `RTICX_EXPAND` is set.
    pub fn build_rtic_macro2(
        mut self,
        args: TokenStream2,
        app_mod: ItemMod,
        source_file: Option<std::path::PathBuf>,
    ) -> TokenStream2 {
        self.core.subscribe(self.info_bus.clone());

        let expand_log = ExpandLog::from_env(source_file);
        let mut args = args;
        let mut app_mod = app_mod;

        // Best-effort: baseline snapshot of the module exactly as the user
        // wrote it, so the first diff shows what the first pass changed.
        if let Some(log) = &expand_log
            && log.pass_dir().is_some()
        {
            let state =
                render_pass_state("original app module (before all passes)", &args, &app_mod);
            log.write_pass_state(0, "original", &state);
        }

        // First, run pre-core passes (in the order of their insertion)
        for (idx, pass) in self.pre_std_passes.iter_mut().enumerate() {
            (*pass).subscribe(self.info_bus.clone());
            // Clone the pre-pass state (only while logging) so that the input
            // of a failing pass can still be dumped.
            let pass_input = expand_log
                .as_ref()
                .filter(|log| log.pass_dir().is_some())
                .map(|_| (args.clone(), app_mod.clone()));
            let (out_args, out_mod) = match pass.run_pass(args, app_mod) {
                Ok(out) => out,
                Err(e) => {
                    // Best-effort: dump the state the failing pass received so
                    // pass developers can inspect what broke it.
                    if let (Some(log), Some((pre_args, pre_mod))) = (&expand_log, pass_input) {
                        let state = render_pass_state(
                            &format!("input to `{}` (pass failed)", pass.pass_name()),
                            &pre_args,
                            &pre_mod,
                        );
                        log.write_pass_state(
                            idx + 1,
                            &format!("{}_input", pass.pass_name()),
                            &state,
                        );
                    }
                    return contextualize(e, format!("in `{}` compilation pass", pass.pass_name()));
                }
            };
            app_mod = out_mod;
            args = out_args;
            // Snapshot the module after the pass so consecutive snapshots can
            // be diffed to see exactly what the pass changed.
            if let Some(log) = &expand_log
                && log.pass_dir().is_some()
            {
                let state = render_pass_state(
                    &format!("output of `{}`", pass.pass_name()),
                    &args,
                    &app_mod,
                );
                log.write_pass_state(idx + 1, pass.pass_name(), &state);
            }
        }

        // parse user application comprised of init, idle, and other tasks and resources
        let parse_input = expand_log
            .as_ref()
            .filter(|log| log.pass_dir().is_some())
            .map(|_| (args.clone(), app_mod.clone()));
        let mut parsed_app = match App::parse(args, app_mod) {
            Ok(parsed) => parsed,
            Err(e) => {
                // Best-effort: dump the post-passes state the core parser
                // received, so pass developers can inspect what broke it.
                if let (Some(log), Some((post_args, post_mod))) = (&expand_log, parse_input) {
                    let state = render_pass_state(
                        "state after all passes (core parsing failed)",
                        &post_args,
                        &post_mod,
                    );
                    log.write_pass_state(self.pre_std_passes.len() + 1, "post_passes", &state);
                }
                return contextualize(
                    e,
                    "in `core` compilation pass during the user code `parsing` phase",
                );
            }
        };
        self.info_bus
            .publish("rticx_core::App", parsed_app.clone())
            .expect("no other pass should publish the entry `rticx_core::App`");

        // update resource ceilings and gather more information about the application
        let analysis = match Analysis::run(&mut parsed_app) {
            Ok(a) => a,
            Err(e) => {
                return contextualize(
                    e,
                    "in `core` compilation pass during the user code `analysis` phase",
                );
            }
        };
        self.info_bus
            .publish("rticx_core::Analysis", analysis.clone())
            .expect("no other pass should publish the entry `rticx_core::Analysis`");

        // Before starting code generation, ask distribution for further checks
        if let Err(e) = self.core.pre_codegen_validation(&parsed_app, &analysis) {
            return e.to_compile_error();
        }

        // Collect injections from all passes
        let mut injections = MainInjections::default();
        for pass in &self.pre_std_passes {
            if let Some(tokens) = pass.main_injection(&MainInjectionPoint::BeforeInit) {
                injections.before_init.push(tokens);
            }
            if let Some(tokens) = pass.main_injection(&MainInjectionPoint::BeforePostInit) {
                injections.before_post_init.push(tokens);
            }
            if let Some(tokens) = pass.main_injection(&MainInjectionPoint::BeforeIdle) {
                injections.before_idle.push(tokens);
            }
        }

        let code = CodeGen::new(self.core.as_ref(), &parsed_app, &analysis)
            .with_injections(&injections)
            .run();

        // Best-effort: write the final expansion even if the generated code
        // later fails to type-check, so it can still be inspected/debugged.
        if let Some(log) = &expand_log {
            log.write("expanded", &code);
            if log.pass_dir().is_some() {
                log.write_pass_state(self.pre_std_passes.len() + 1, "core", &code.to_string());
            }
        }

        code
    }

    pub fn info_bus(&self) -> &InfoBus {
        &self.info_bus
    }
}

/// Wrap an error with context about which compilation pass/phase failed,
/// keeping the original span so rustc points at the user's code.
fn contextualize(e: syn::Error, context: impl std::fmt::Display) -> TokenStream2 {
    syn::Error::new(e.span(), format!("{context}: {e}")).to_compile_error()
}
