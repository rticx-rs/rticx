//! A mock implementation of [CorePassBackend] for testing.
//!
//! It is intended for use in `rtic-core`
//! integration tests and in tests of downstream compilation passes and distributions that need a
//! backend stand-in.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_quote, Ident};

use crate::analysis::SubAnalysis;
use crate::parser::ast::AppArgs;
use crate::parser::SubApp;
use crate::{Analysis, App, CorePassBackend};

/// A no-op backend used for testing the parser, analysis, and codegen pieces of `rtic-core`.
///
/// The mock returns deterministic stubs for code-generation hooks so that tests can verify the
/// shape of the expanded output without needing real target hardware bindings.
pub struct MockCoreBackend;

impl CorePassBackend for MockCoreBackend {
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        None
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        mut incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        incomplete_lock_fn.block = parse_quote! {
            {
                // mock backend: lock implementation
                f(unsafe { &mut *resource_ptr })
            }
        };
        incomplete_lock_fn
    }

    fn generate_global_definitions(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        Some(quote! {
            // mock backend: global definitions
        })
    }

    fn wrap_task_execution(
        &self,
        _task_prio: u16,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        Some(dispatch_task_call)
    }

    fn entry_name(&self, _core: u32) -> Ident {
        format_ident!("main")
    }

    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        None
    }

    fn generate_interrupt_free_fn(&self, mut empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        empty_body_fn.block = parse_quote! {
            {
                // mock backend: interrupt-free critical section
                f()
            }
        };
        empty_body_fn
    }

    fn pre_codegen_validation(&self, _app: &App, _analysis: &Analysis) -> syn::Result<()> {
        Ok(())
    }

    fn default_task_priority(&self) -> u16 {
        1
    }

    #[cfg(feature = "multibin")]
    fn multibin_shared_macro_path(&self) -> syn::Path {
        parse_quote!(crate::mock_shared)
    }
}

impl Default for MockCoreBackend {
    fn default() -> Self {
        Self
    }
}
